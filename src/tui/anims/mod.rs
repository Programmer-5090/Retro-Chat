mod cube_anim;
mod matrix_rain_anim;
mod starfield_anim;
mod torus_anim;
mod sand_sim_anim;
// mod spectrum;

pub use cube_anim::SpinningCube;
pub use matrix_rain_anim::MatrixRain;
pub use starfield_anim::Starfield;
pub use torus_anim::SpinningTorus;
pub use sand_sim_anim::SandSim;
// pub use spectrum::SpectrumState;

pub const TAP_BUFFER_CAPACITY: usize = 4096;
