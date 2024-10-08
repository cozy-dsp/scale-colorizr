#![feature(portable_simd)]
#![feature(array_windows)]
#![warn(clippy::pedantic, clippy::nursery)]

mod editor;
mod spectrum;

use cozy_util::filter::svf::GenericSVF;
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use spectrum::{SpectrumInput, SpectrumOutput};
use std::simd::f32x2;
use std::sync::Arc;

const MAX_BLOCK_SIZE: usize = 64;
pub const NUM_VOICES: usize = 128;
pub const NUM_FILTERS: usize = 8;

pub type FrequencyDisplay = [[AtomicCell<Option<f32>>; NUM_FILTERS]; NUM_VOICES];
pub type FilterDisplay = [[AtomicCell<Option<GenericSVF<f32x2>>>; NUM_FILTERS]; NUM_VOICES];

pub const VERSION: &str = env!("VERGEN_GIT_DESCRIBE");

#[derive(Clone)]
struct Voice {
    id: i32,
    channel: u8,
    note: u8,
    frequency: f32,
    internal_voice_id: u64,
    velocity_sqrt: f32,
    filters: [GenericSVF<f32x2>; NUM_FILTERS],
    releasing: bool,
    amp_envelope: Smoother<f32>,
}

pub struct ScaleColorizr {
    params: Arc<ScaleColorizrParams>,
    voices: [Option<Voice>; NUM_VOICES],
    dry_signal: [f32x2; MAX_BLOCK_SIZE],
    frequency_display: Arc<FrequencyDisplay>,
    filter_display: Arc<FilterDisplay>,
    sample_rate: Arc<AtomicF32>,
    midi_event_debug: Arc<AtomicCell<Option<NoteEvent<()>>>>,
    next_internal_voice_id: u64,
    pre_spectrum_input: SpectrumInput,
    pre_spectrum_output: Option<SpectrumOutput>,
    post_spectrum_input: SpectrumInput,
    post_spectrum_output: Option<SpectrumOutput>,
}

#[derive(Enum, PartialEq)]
enum FilterMode {
    Peak,
    Notch,
}

#[derive(Params)]
struct ScaleColorizrParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "attack"]
    pub attack: FloatParam,
    #[id = "release"]
    pub release: FloatParam,
    #[id = "band-width"]
    pub band_width: FloatParam,
    #[id = "delta"]
    pub delta: BoolParam,
    #[id = "safety-switch"]
    pub safety_switch: BoolParam,
    #[id = "voice-count"]
    pub voice_count: IntParam,
    #[id = "filter-mod"]
    pub filter_mode: EnumParam<FilterMode>,
}

impl Default for ScaleColorizr {
    fn default() -> Self {
        let (pre_spectrum_input, pre_spectrum_output) = SpectrumInput::new(2);
        let (post_spectrum_input, post_spectrum_output) = SpectrumInput::new(2);

        Self {
            params: Arc::new(ScaleColorizrParams::default()),
            // TODO: this feels dumb
            voices: [0; NUM_VOICES].map(|_| None),
            dry_signal: [f32x2::default(); MAX_BLOCK_SIZE],
            frequency_display: Arc::new(core::array::from_fn(|_| {
                core::array::from_fn(|_| AtomicCell::default())
            })),
            filter_display: Arc::new(core::array::from_fn(|_| {
                core::array::from_fn(|_| AtomicCell::default())
            })),
            sample_rate: Arc::new(AtomicF32::new(1.0)),
            midi_event_debug: Arc::new(AtomicCell::new(None)),
            next_internal_voice_id: 0,
            pre_spectrum_input,
            pre_spectrum_output: Some(pre_spectrum_output),
            post_spectrum_input,
            post_spectrum_output: Some(post_spectrum_output),
        }
    }
}

impl Default for ScaleColorizrParams {
    fn default() -> Self {
        Self {
            editor_state: editor::default_editor_state(),
            gain: FloatParam::new(
                "Band Gain",
                10.0,
                FloatRange::Linear {
                    min: 2.0,
                    max: 40.0,
                },
            )
            .with_step_size(0.1)
            .with_unit(" dB"),
            attack: FloatParam::new(
                "Attack",
                2.0,
                FloatRange::Linear {
                    min: 2.0,
                    max: 2000.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),
            release: FloatParam::new(
                "Release",
                10.0,
                FloatRange::Linear {
                    min: 2.0,
                    max: 2000.0,
                },
            )
            .with_unit(" ms")
            .with_step_size(0.1),

            band_width: FloatParam::new(
                "Band Width",
                100.0,
                FloatRange::Linear {
                    min: 100.0,
                    max: 300.0,
                },
            )
            .with_unit("%")
            .with_step_size(0.1),

            delta: BoolParam::new("Delta", false),
            safety_switch: BoolParam::new("SAFETY SWITCH", true).hide(),
            #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            voice_count: IntParam::new(
                "Voices",
                16,
                IntRange::Linear {
                    min: 1,
                    max: NUM_VOICES as i32,
                },
            ),
            filter_mode: EnumParam::new("Filter Mode", FilterMode::Peak),
        }
    }
}

impl Plugin for ScaleColorizr {
    const NAME: &'static str = "Scale Colorizr";
    const VENDOR: &'static str = "cozy dsp";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "hi@cozydsp.space";

    const VERSION: &'static str = VERSION;

    // The first audio IO layout is used as the default. The other layouts may be selected either
    // explicitly or automatically by the host or the user depending on the plugin API/backend.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),

        aux_input_ports: &[],
        aux_output_ports: &[],

        // Individual ports and the layout as a whole can be named here. By default these names
        // are generated as needed. This layout will be called 'Stereo', while a layout with
        // only one input and output channel would be called 'Mono'.
        names: PortNames::const_default(),
    }];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    // If the plugin can send or receive SysEx messages, it can define a type to wrap around those
    // messages here. The type implements the `SysExMessage` trait, which allows conversion to and
    // from plain byte buffers.
    type SysExMessage = ();
    // More advanced plugins can use this to run expensive background tasks. See the field's
    // documentation for more information. `()` means that the plugin does not have any background
    // tasks.
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(
            self.params.clone(),
            self.frequency_display.clone(),
            self.pre_spectrum_output.take().expect("either the pre spectrum didn't initialize properly, or the editor is being queried twice. either way, something has gone horribly wrong"),
            self.post_spectrum_output.take().expect("either the post spectrum didn't initialize properly, or the editor is being queried twice. either way, something has gone horribly wrong"),
            self.sample_rate.clone(),
            self.midi_event_debug.clone(),
            self.filter_display.clone(),
        )
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate.store(
            buffer_config.sample_rate,
            std::sync::atomic::Ordering::Relaxed,
        );

        self.pre_spectrum_input
            .update_sample_rate(buffer_config.sample_rate);
        self.post_spectrum_input
            .update_sample_rate(buffer_config.sample_rate);

        true
    }

    fn reset(&mut self) {
        for voice in &mut self.voices {
            if voice.is_some() {
                *voice = None;
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // NIH-plug has a block-splitting adapter for `Buffer`. While this works great for effect
        // plugins, for polyphonic synths the block size should be `min(MAX_BLOCK_SIZE,
        // num_remaining_samples, next_event_idx - block_start_idx)`. Because blocks also need to be
        // split on note events, it's easier to work with raw audio here and to do the splitting by
        // hand.

        if self.params.editor_state.is_open() {
            self.pre_spectrum_input.compute(buffer);
        }

        let num_samples = buffer.samples();
        let sample_rate = self.sample_rate.load(std::sync::atomic::Ordering::Relaxed);
        let output = buffer.as_slice();

        let mut next_event = context.next_event();
        let mut block_start: usize = 0;
        let mut block_end: usize = MAX_BLOCK_SIZE.min(num_samples);
        while block_start < num_samples {
            self.process_events(
                &mut next_event,
                block_start,
                sample_rate,
                context,
                &mut block_end,
            );

            // These are the smoothed global parameter values. These are used for voices that do not
            // have polyphonic modulation applied to them. With a plugin as simple as this it would
            // be possible to avoid this completely by simply always copying the smoother into the
            // voice's struct, but that may not be realistic when the plugin has hundreds of
            // parameters. The `voice_*` arrays are scratch arrays that an individual voice can use.
            let block_len = block_end - block_start;
            let mut gain = [0.0; MAX_BLOCK_SIZE];
            let mut voice_amp_envelope = [0.0; MAX_BLOCK_SIZE];
            self.params.gain.smoothed.next_block(&mut gain, block_len);

            for (value_idx, sample_idx) in (block_start..block_end).enumerate() {
                self.dry_signal[value_idx] =
                    f32x2::from_array([output[0][sample_idx], output[1][sample_idx]]);
            }

            for voice in self.voices.iter_mut().filter_map(|v| v.as_mut()) {
                voice
                    .amp_envelope
                    .next_block(&mut voice_amp_envelope, block_len);

                for (value_idx, sample_idx) in (block_start..block_end).enumerate() {
                    let amp = gain[value_idx] * voice.velocity_sqrt * voice_amp_envelope[value_idx];
                    let mut sample =
                        f32x2::from_array([output[0][sample_idx], output[1][sample_idx]]);

                    for (filter_idx, filter) in voice.filters.iter_mut().enumerate() {
                        #[allow(clippy::cast_precision_loss)]
                        let frequency = voice.frequency * (filter_idx as f32 + 1.0);

                        if self.params.safety_switch.value() && frequency >= sample_rate / 2.0 {
                            continue;
                        }

                        #[allow(clippy::cast_precision_loss)]
                        let adjusted_frequency = (frequency - voice.frequency)
                            / (voice.frequency * (NUM_FILTERS / 2) as f32);
                        let amp_falloff = (-adjusted_frequency).exp();
                        filter.set_sample_rate(sample_rate);

                        let q = 39.0f32
                            .mul_add(-self.params.band_width.modulated_normalized_value(), 40.0);

                        match self.params.filter_mode.value() {
                            FilterMode::Peak => filter.set_bell(frequency, q, amp * amp_falloff),
                            FilterMode::Notch => filter.set_notch(frequency, q),
                        };

                        sample = filter.process(sample);
                    }

                    output[0][sample_idx] = sample.as_array()[0];
                    output[1][sample_idx] = sample.as_array()[1];
                }
            }

            if self.params.delta.value() {
                for (value_idx, sample_idx) in (block_start..block_end).enumerate() {
                    let mut sample =
                        f32x2::from_array([output[0][sample_idx], output[1][sample_idx]]);
                    sample += self.dry_signal[value_idx] * f32x2::splat(-1.0);

                    output[0][sample_idx] = sample.as_array()[0];
                    output[1][sample_idx] = sample.as_array()[1];
                }
            }

            // Terminate voices whose release period has fully ended. This could be done as part of
            // the previous loop but this is simpler.
            for voice in &mut self.voices {
                match voice {
                    Some(v) if v.releasing && v.amp_envelope.previous_value() == 0.0 => {
                        // This event is very important, as it allows the host to manage its own modulation
                        // voices
                        #[allow(clippy::cast_possible_truncation)]
                        context.send_event(NoteEvent::VoiceTerminated {
                            timing: block_end as u32,
                            voice_id: Some(v.id),
                            channel: v.channel,
                            note: v.note,
                        });
                        *voice = None;
                    }
                    _ => (),
                }
            }

            // And then just keep processing blocks until we've run out of buffer to fill
            block_start = block_end;
            block_end = (block_start + MAX_BLOCK_SIZE).min(num_samples);
        }

        if self.params.editor_state.is_open() {
            for (voice, displays) in self.voices.iter().zip(self.frequency_display.iter()) {
                if let Some(voice) = voice {
                    for (voice_filter, display) in voice.filters.iter().zip(displays) {
                        display.store(Some(voice_filter.frequency()));
                    }
                } else {
                    for display in displays {
                        display.store(None);
                    }
                }
            }

            for (voice, displays) in self.voices.iter().zip(self.filter_display.iter()) {
                if let Some(voice) = voice {
                    for (voice_filter, display) in voice.filters.iter().zip(displays) {
                        display.store(Some(*voice_filter));
                    }
                } else {
                    for display in displays {
                        display.store(None);
                    }
                }
            }

            self.post_spectrum_input.compute(buffer);
        }

        ProcessStatus::Normal
    }
}

impl ScaleColorizr {
    /// Start a new voice with the given voice ID. If all voices are currently in use, the oldest
    /// voice will be stolen. Returns a reference to the new voice.
    fn start_voice(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        sample_offset: u32,
        voice_id: Option<i32>,
        channel: u8,
        note: u8,
    ) -> &mut Voice {
        #[allow(clippy::cast_precision_loss)]
        let freq = util::midi_note_to_freq(note) / (NUM_FILTERS / 2) as f32;
        let new_voice = Voice {
            id: voice_id.unwrap_or_else(|| compute_fallback_voice_id(note, channel)),
            internal_voice_id: self.next_internal_voice_id,
            channel,
            note,
            frequency: freq,
            velocity_sqrt: 1.0,

            releasing: false,
            amp_envelope: Smoother::none(),

            filters: [GenericSVF::default(); NUM_FILTERS],
        };
        self.next_internal_voice_id = self.next_internal_voice_id.wrapping_add(1);

        #[allow(clippy::cast_sign_loss)]
        if let Some(free_voice_idx) = self
            .voices
            .iter()
            .take(self.params.voice_count.value() as usize)
            .position(Option::is_none)
        {
            self.voices[free_voice_idx] = Some(new_voice);
            return self.voices[free_voice_idx].as_mut().unwrap();
        }
        // If there is no free voice, find and steal the oldest one
        // SAFETY: We can skip a lot of checked unwraps here since we already know all voices are in
        //         use
        let oldest_voice = unsafe {
            #[allow(clippy::cast_sign_loss)]
            self.voices
                .iter_mut()
                .take(self.params.voice_count.value() as usize)
                .min_by_key(|voice| voice.as_ref().unwrap_unchecked().internal_voice_id)
                .unwrap_unchecked()
        };

        // The stolen voice needs to be terminated so the host can reuse its modulation
        // resources
        {
            let oldest_voice = oldest_voice.as_ref().unwrap();
            context.send_event(NoteEvent::VoiceTerminated {
                timing: sample_offset,
                voice_id: Some(oldest_voice.id),
                channel: oldest_voice.channel,
                note: oldest_voice.note,
            });
        }

        *oldest_voice = Some(new_voice);
        return oldest_voice.as_mut().unwrap();
    }

    /// Start the release process for one or more voice by changing their amplitude envelope. If
    /// `voice_id` is not provided, then this will terminate all matching voices.
    fn start_release_for_voices(
        &mut self,
        sample_rate: f32,
        voice_id: Option<i32>,
        channel: u8,
        note: u8,
    ) {
        for voice in self
            .voices
            .iter_mut()
            .filter_map(|v| {
                v.as_mut().filter(|v| {
                    voice_id.is_some_and(|id| v.id == id)
                        || (v.channel == channel && v.note == note)
                })
            })
            // if we were provided with a voice id, take the first thing in the iterator. otherwise, all of em
            .take(voice_id.map_or(usize::MAX, |_| 1))
        {
            voice.releasing = true;
            voice.amp_envelope.style = SmoothingStyle::Exponential(self.params.release.value());
            voice.amp_envelope.set_target(sample_rate, 0.0);
        }
    }

    /// Immediately terminate one or more voice, removing it from the pool and informing the host
    /// that the voice has ended. If `voice_id` is not provided, then this will terminate all
    /// matching voices.
    fn choke_voices(
        &mut self,
        context: &mut impl ProcessContext<Self>,
        sample_offset: u32,
        voice_id: Option<i32>,
        channel: u8,
        note: u8,
    ) {
        for voice in self
            .voices
            .iter_mut()
            .filter(|v| {
                v.as_ref().is_some_and(|v| {
                    voice_id.is_some_and(|id| v.id == id)
                        || (v.channel == channel && v.note == note)
                })
            })
            // if we were provided with a voice id, take the first thing in the iterator. otherwise, all of em
            .take(voice_id.map_or(usize::MAX, |_| 1))
        {
            if let Some(voice) = voice.take() {
                context.send_event(NoteEvent::VoiceTerminated {
                    timing: sample_offset,
                    // Notice how we always send the terminated voice ID here
                    voice_id: Some(voice.id),
                    channel,
                    note,
                });
            }
        }
    }

    fn retune_voice(&mut self, voice_id: Option<i32>, channel: u8, note: u8, tuning: f32) {
        if let Some(voice) = self
            .voices
            .iter_mut()
            .filter_map(|v| v.as_mut())
            .find(|v| voice_id == Some(v.id) || (v.channel == channel && v.note == note))
        {
            voice.frequency = util::f32_midi_note_to_freq(f32::from(note) + tuning);
        }
    }

    fn process_events(
        &mut self,
        next_event: &mut Option<NoteEvent<()>>,
        block_start: usize,
        sample_rate: f32,
        context: &mut impl ProcessContext<Self>,
        block_end: &mut usize,
    ) {
        // First of all, handle all note events that happen at the start of the block, and cut
        // the block short if another event happens before the end of it.
        loop {
            match *next_event {
                // If the event happens now, then we'll keep processing events
                Some(event) if (event.timing() as usize) <= block_start => {
                    self.midi_event_debug.store(Some(event));
                    // This synth doesn't support any of the polyphonic expression events. A
                    // real synth plugin however will want to support those.
                    match event {
                        NoteEvent::NoteOn {
                            timing,
                            voice_id,
                            channel,
                            note,
                            velocity,
                        } => {
                            // This starts with the attack portion of the amplitude envelope
                            let amp_envelope = Smoother::new(SmoothingStyle::Exponential(
                                self.params.attack.value(),
                            ));
                            amp_envelope.reset(0.0);
                            amp_envelope.set_target(sample_rate, 1.0);

                            let voice = self.start_voice(context, timing, voice_id, channel, note);
                            voice.velocity_sqrt = velocity.sqrt();
                            voice.amp_envelope = amp_envelope;
                        }
                        NoteEvent::NoteOff {
                            timing: _,
                            voice_id,
                            channel,
                            note,
                            velocity: _,
                        } => {
                            self.start_release_for_voices(sample_rate, voice_id, channel, note);
                        }
                        NoteEvent::Choke {
                            timing,
                            voice_id,
                            channel,
                            note,
                        } => {
                            self.choke_voices(context, timing, voice_id, channel, note);
                        }
                        NoteEvent::PolyTuning {
                            voice_id,
                            channel,
                            note,
                            tuning,
                            ..
                        } => {
                            self.retune_voice(voice_id, channel, note, tuning);
                        }
                        _ => {}
                    };

                    *next_event = context.next_event();
                }
                // If the event happens before the end of the block, then the block should be cut
                // short so the next block starts at the event
                Some(event) if (event.timing() as usize) < *block_end => {
                    *block_end = event.timing() as usize;
                    return;
                }
                _ => return,
            }
        }
    }
}

/// Compute a voice ID in case the host doesn't provide them.
const fn compute_fallback_voice_id(note: u8, channel: u8) -> i32 {
    note as i32 | ((channel as i32) << 16)
}

impl ClapPlugin for ScaleColorizr {
    const CLAP_ID: &'static str = "space.cozydsp.scale_colorizr";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Filter based sound colorizer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;

    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Filter,
    ];
}

impl Vst3Plugin for ScaleColorizr {
    const VST3_CLASS_ID: [u8; 16] = *b"COZYDSP_SCLECLZR";

    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Filter];
}

nih_export_clap!(ScaleColorizr);
nih_export_vst3!(ScaleColorizr);
