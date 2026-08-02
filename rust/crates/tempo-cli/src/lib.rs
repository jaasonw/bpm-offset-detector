//! Library surface of `tempo-cli`: shares the audio decoding logic
//! between the `tempo-cli` binary and auxiliary binaries (e.g.
//! `osu-eval`, the osu!-map ground-truth comparison harness).

pub mod decode;
pub mod osu;
