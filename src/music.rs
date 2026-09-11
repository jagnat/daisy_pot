
use micromath::F32Ext;
use rand_xoshiro::{
    Xoshiro128PlusPlus,
    rand_core::{Rng, SeedableRng},
};

struct NoteCommand {
}

#[derive(PartialEq, Eq)]
enum EnvStage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

struct Envelope {
    stage: EnvStage,
    level: f32,
    sample_rate: f32,
    attack_rate: f32,
    decay_rate: f32,
    release_rate: f32,
    sustain_level: f32,
}

impl Envelope {
    fn new(sample_rate: f32) -> Envelope {
        Envelope {
            stage: EnvStage::Idle,
            level: 0.0,
            sample_rate: sample_rate,
            attack_rate: 0.0,
            decay_rate: 0.0,
            release_rate: 0.0,
            sustain_level: 1.0,
        }
    }

    fn reset(&mut self) {
        self.stage = EnvStage::Idle;
        self.level = 0.0;
    }

    fn set_params(&mut self, a: f32, d: f32, s: f32, r: f32) {
        self.sustain_level = s;
        if a > 0.0 { 
            self.attack_rate = 1.0 / (a * self.sample_rate);
        } else {
            self.attack_rate = 1.0 // instant
        }

        if d > 0.0 {
            self.decay_rate = F32Ext::exp(-6.9078 / (d * self.sample_rate));
        } else {
            self.decay_rate = 0.0;
        }

        if r > 0.0 {
            self.release_rate = F32Ext::exp(-6.9078 / (r * self.sample_rate));
        } else {
            self.release_rate = 0.0;
        }
    }

    fn gate_on(&mut self) {
        self.stage = EnvStage::Attack;
    }

    fn gate_off(&mut self) {
        if self.stage != EnvStage::Idle {
            self.stage = EnvStage::Release;
        }
    }

    fn next(&mut self) -> f32 {
        match self.stage {
            EnvStage::Idle => self.level = 0.0,
            EnvStage::Attack => {
                self.level += self.attack_rate;
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = EnvStage::Decay;
                }
            },
            EnvStage::Decay => {
                self.level = self.sustain_level + (self.level - self.sustain_level) * self.decay_rate;
                if F32Ext::abs(self.level - self.sustain_level) < 1e-5 {
                    self.level = self.sustain_level;
                    self.stage = EnvStage::Sustain;
                }
            },
            EnvStage::Sustain => self.level = self.sustain_level,
            EnvStage::Release => {
                self.level *= self.release_rate;
                if self.level < 1e-5 {
                    self.level = 0.0;
                    self.stage = EnvStage::Idle;
                }
            }
        };
        self.level
    }

    fn fill(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.next();
        }
    }
}

#[derive(PartialEq, Eq)]
enum OscWave {
    Sine,
    Saw,
    Triangle,
    Square,
    Noise,
}

struct Osc {
    phase: f32,
    phase_inc: f32,
    sample_rate: f32,
    wave: OscWave,

    // counters
    val: f32,
    last_val: f32,
    tri_acc: f32,
    noise_rng: Xoshiro128PlusPlus,
}

impl Osc {
    fn new(sample_rate: f32) -> Osc {
        Osc {
            phase: 0.0,
            phase_inc: 0.0,
            sample_rate: sample_rate,
            wave: OscWave::Sine,
            val: 0.0,
            last_val: 0.0,
            tri_acc: 0.0,
            noise_rng: Xoshiro128PlusPlus::seed_from_u64(0xDA15_5EED),
        }
    }

    fn set_freq(&mut self, freq: f32) {
        self.phase_inc = freq / self.sample_rate;
    }

    fn reset(&mut self) {
        self.phase = 0.0;
    }

    fn next(&mut self) -> f32 {
        self.last_val = self.val;
        self.val = self.polyblep_value();
        self.phase += self.phase_inc;
        self.phase -= (self.phase as i32) as f32;
        if self.phase < 0.0 {
            self.phase += 1.0;
        }
        self.val
    }
    
    fn fill(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.next();
        }
    }

    fn blep(t: f32, dt: f32) -> f32 {
        if t < dt {
            return - ((t / dt - 1.0) * (t / dt - 1.0));
        } else if t > 1.0 - dt {
            return ((t - 1.0) / dt + 1.0) * ((t - 1.0) / dt + 1.0);
        } else {
            return 0.0;
        }
    }

    fn polyblep_value(&mut self) -> f32 {
        let mut val: f32;
        let phase = self.phase;
        let phase_inc = self.phase_inc;
        match self.wave {
            OscWave::Sine => val = F32Ext::sin(core::f32::consts::TAU * phase),
            OscWave::Saw => val = (2.0 * phase - 1.0) - Osc::blep(phase, phase_inc),
            OscWave::Square | OscWave::Triangle => {
                val = if phase < 0.5 { 1.0 } else {-1.0 };
                val += Osc::blep(phase, phase_inc);
                val -= Osc::blep((phase + 0.5) % 1.0, phase_inc);
                if self.wave == OscWave::Triangle {
                    self.tri_acc = phase_inc * val + (1.0 - phase_inc) * self.tri_acc;
                    val = self.tri_acc * 4.0;
                }
            },
            OscWave::Noise => {
                const SCALE: f32 = 2.0 / 16_777_216.0;
                val = (self.noise_rng.next_u32() >> 8) as f32 * SCALE - 1.0;
            },
        }
        val
    }
}
