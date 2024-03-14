#![feature(portable_simd)]

mod editor;
mod filter;

use crate::editor::create_editor;
use crate::filter::{Biquad, BiquadCoefficients};
use crossbeam::atomic::AtomicCell;
use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use std::simd::f32x2;
use std::sync::Arc;

const MAX_BLOCK_SIZE: usize = 64;
pub const NUM_VOICES: u32 = 16;
pub const NUM_FILTERS: usize = 8;

pub type FrequencyDisplay = [[AtomicCell<Option<f32>>; NUM_FILTERS]; NUM_VOICES as usize];
pub type BiquadDisplay = [[AtomicCell<Option<BiquadCoefficients<f32x2>>>; NUM_FILTERS]; NUM_VOICES as usize];

#[derive(Clone)]
struct Voice {
    voice_id: i32,
    channel: u8,
    note: u8,
    frequency: f32,
    internal_voice_id: u64,
    velocity_sqrt: f32,
    filters: [Biquad<f32x2>; NUM_FILTERS],
    releasing: bool,
    amp_envelope: Smoother<f32>,
}

struct ScaleColorizr {
    params: Arc<ScaleColorizrParams>,
    voices: [Option<Voice>; NUM_VOICES as usize],
    dry_signal: [f32x2; MAX_BLOCK_SIZE],
    frequency_display: Arc<FrequencyDisplay>,
    biquad_display: Arc<BiquadDisplay>,
    sample_rate: Arc<AtomicF32>,
    next_internal_voice_id: u64,
}

#[derive(Params)]
struct ScaleColorizrParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,

    #[id = "gain"]
    pub gain: FloatParam,
    #[id = "delta"]
    pub delta: BoolParam,
}

impl Default for ScaleColorizr {
    fn default() -> Self {
        Self {
            params: Arc::new(ScaleColorizrParams::default()),
            // TODO: this feels dumb
            voices: [0; NUM_VOICES as usize].map(|_| None),
            dry_signal: [f32x2::default(); MAX_BLOCK_SIZE],
            frequency_display: Arc::new(core::array::from_fn(|_| {
                core::array::from_fn(|_| AtomicCell::default())
            })),
            biquad_display: Arc::new(core::array::from_fn(|_| {
                core::array::from_fn(|_| AtomicCell::default())
            })),
            sample_rate: Arc::new(AtomicF32::new(1.0)),
            next_internal_voice_id: 0,
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
            .with_smoother(SmoothingStyle::Logarithmic(50.0))
            .with_value_to_string(formatters::v2s_f32_gain_to_db(2))
            .with_string_to_value(formatters::s2v_f32_gain_to_db())
            .with_unit(" dB"),
            delta: BoolParam::new("Delta", false),
        }
    }
}

impl Plugin for ScaleColorizr {
    const NAME: &'static str = "Scale Colorizr";
    const VENDOR: &'static str = "METALWINGS DSP";
    const URL: &'static str = env!("CARGO_PKG_HOMEPAGE");
    const EMAIL: &'static str = "hello@murl.digital";

    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

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

    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
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
        create_editor(
            self.params.editor_state.clone(),
            self.sample_rate.clone(),
            self.params.clone(),
            self.frequency_display.clone(),
            self.biquad_display.clone()
        )
    }

    fn initialize(
            &mut self,
            audio_io_layout: &AudioIOLayout,
            buffer_config: &BufferConfig,
            context: &mut impl InitContext<Self>,
        ) -> bool {
        self.sample_rate.store(buffer_config.sample_rate, std::sync::atomic::Ordering::Relaxed);
        true
    }

    fn reset(&mut self) {
        for voice in self.voices.iter_mut() {
            if voice.is_some() {
                *voice = None;
            }
        }
    }

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
        let num_samples = buffer.samples();
        let sample_rate = context.transport().sample_rate;
        let output = buffer.as_slice();

        let mut next_event = context.next_event();
        let mut block_start: usize = 0;
        let mut block_end: usize = MAX_BLOCK_SIZE.min(num_samples);
        while block_start < num_samples {
            // First of all, handle all note events that happen at the start of the block, and cut
            // the block short if another event happens before the end of it.
            'events: loop {
                match next_event {
                    // If the event happens now, then we'll keep processing events
                    Some(event) if (event.timing() as usize) <= block_start => {
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
                                let amp_envelope = Smoother::new(SmoothingStyle::Exponential(20.0));
                                amp_envelope.reset(0.0);
                                amp_envelope.set_target(sample_rate, 1.0);

                                let voice = self.start_voice(
                                    context,
                                    timing,
                                    voice_id,
                                    channel,
                                    note,
                                );
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
                                self.start_release_for_voices(sample_rate, voice_id, channel, note)
                            }
                            NoteEvent::Choke {
                                timing,
                                voice_id,
                                channel,
                                note,
                            } => {
                                self.choke_voices(context, timing, voice_id, channel, note);
                            }
                            _ => (),
                        };

                        next_event = context.next_event();
                    }
                    // If the event happens before the end of the block, then the block should be cut
                    // short so the next block starts at the event
                    Some(event) if (event.timing() as usize) < block_end => {
                        block_end = event.timing() as usize;
                        break 'events;
                    }
                    _ => break 'events,
                }
            }

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
                        let frequency = voice.frequency * (filter_idx as f32 + 1.0);
                        let adjusted_frequency = (frequency - voice.frequency) / (voice.frequency * (NUM_FILTERS/2) as f32);
                        let amp_falloff = (-adjusted_frequency).exp();
                        filter.coefficients =
                            BiquadCoefficients::peaking_eq(sample_rate, frequency, amp * amp_falloff, 40.0);
                        filter.frequency = frequency;
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
            for voice in self.voices.iter_mut() {
                match voice {
                    Some(v) if v.releasing && v.amp_envelope.previous_value() == 0.0 => {
                        // This event is very important, as it allows the host to manage its own modulation
                        // voices
                        context.send_event(NoteEvent::VoiceTerminated {
                            timing: block_end as u32,
                            voice_id: Some(v.voice_id),
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
                        display.store(Some(voice_filter.frequency));
                    }
                } else {
                    for display in displays {
                        display.store(None);
                    }
                }
            }

            for (voice, displays) in self.voices.iter().zip(self.biquad_display.iter()) {
                if let Some(voice) = voice {
                    for (voice_filter, display) in voice.filters.iter().zip(displays) {
                        display.store(Some(voice_filter.coefficients));
                    }
                } else {
                    for display in displays {
                        display.store(None);
                    }
                }
            }
        }

        ProcessStatus::Normal
    }
}

impl ScaleColorizr {
    /// Get the index of a voice by its voice ID, if the voice exists. This does not immediately
    /// reutnr a reference to the voice to avoid lifetime issues.
    fn get_voice_idx(&mut self, voice_id: i32) -> Option<usize> {
        self.voices
            .iter_mut()
            .position(|voice| matches!(voice, Some(voice) if voice.voice_id == voice_id))
    }

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
        let freq = util::midi_note_to_freq(note) / (NUM_FILTERS / 2) as f32;
        let new_voice = Voice {
            voice_id: voice_id.unwrap_or_else(|| compute_fallback_voice_id(note, channel)),
            internal_voice_id: self.next_internal_voice_id,
            channel,
            note,
            frequency: freq,
            velocity_sqrt: 1.0,

            releasing: false,
            amp_envelope: Smoother::none(),

            filters: [Biquad::default(); NUM_FILTERS],
        };
        self.next_internal_voice_id = self.next_internal_voice_id.wrapping_add(1);

        // Can't use `.iter_mut().find()` here because nonlexical lifetimes don't apply to return
        // values
        match self.voices.iter().position(|voice| voice.is_none()) {
            Some(free_voice_idx) => {
                self.voices[free_voice_idx] = Some(new_voice);
                return self.voices[free_voice_idx].as_mut().unwrap();
            }
            None => {
                // If there is no free voice, find and steal the oldest one
                // SAFETY: We can skip a lot of checked unwraps here since we already know all voices are in
                //         use
                let oldest_voice = unsafe {
                    self.voices
                        .iter_mut()
                        .min_by_key(|voice| voice.as_ref().unwrap_unchecked().internal_voice_id)
                        .unwrap_unchecked()
                };

                // The stolen voice needs to be terminated so the host can reuse its modulation
                // resources
                {
                    let oldest_voice = oldest_voice.as_ref().unwrap();
                    context.send_event(NoteEvent::VoiceTerminated {
                        timing: sample_offset,
                        voice_id: Some(oldest_voice.voice_id),
                        channel: oldest_voice.channel,
                        note: oldest_voice.note,
                    });
                }

                *oldest_voice = Some(new_voice);
                return oldest_voice.as_mut().unwrap();
            }
        }
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
        for voice in self.voices.iter_mut() {
            match voice {
                Some(Voice {
                    voice_id: candidate_voice_id,
                    channel: candidate_channel,
                    note: candidate_note,
                    releasing,
                    amp_envelope,
                    ..
                }) if voice_id == Some(*candidate_voice_id)
                    || (channel == *candidate_channel && note == *candidate_note) =>
                {
                    *releasing = true;
                    amp_envelope.style = SmoothingStyle::Exponential(10.0);
                    amp_envelope.set_target(sample_rate, 0.0);

                    // If this targetted a single voice ID, we're done here. Otherwise there may be
                    // multiple overlapping voices as we enabled support for that in the
                    // `PolyModulationConfig`.
                    if voice_id.is_some() {
                        return;
                    }
                }
                _ => (),
            }
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
        for voice in self.voices.iter_mut() {
            match voice {
                Some(Voice {
                    voice_id: candidate_voice_id,
                    channel: candidate_channel,
                    note: candidate_note,
                    ..
                }) if voice_id == Some(*candidate_voice_id)
                    || (channel == *candidate_channel && note == *candidate_note) =>
                {
                    context.send_event(NoteEvent::VoiceTerminated {
                        timing: sample_offset,
                        // Notice how we always send the terminated voice ID here
                        voice_id: Some(*candidate_voice_id),
                        channel,
                        note,
                    });
                    *voice = None;

                    if voice_id.is_some() {
                        return;
                    }
                }
                _ => (),
            }
        }
    }
}

/// Compute a voice ID in case the host doesn't provide them.
const fn compute_fallback_voice_id(note: u8, channel: u8) -> i32 {
    note as i32 | ((channel as i32) << 16)
}

impl ClapPlugin for ScaleColorizr {
    const CLAP_ID: &'static str = "dsp.metalwings.scale_colorizr";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Filter based sound colorizer");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;

    // Don't forget to change these features
    const CLAP_FEATURES: &'static [ClapFeature] = &[ClapFeature::AudioEffect, ClapFeature::Stereo];
}

impl Vst3Plugin for ScaleColorizr {
    const VST3_CLASS_ID: [u8; 16] = *b"DMTLWNGSSCLECLZR";

    // And also don't forget to change these categories
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Dynamics];
}

nih_export_clap!(ScaleColorizr);
nih_export_vst3!(ScaleColorizr);
