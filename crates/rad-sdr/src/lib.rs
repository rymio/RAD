//! RAD's software radio for an RTL-SDR dongle: a spectrum and waterfall, a scanner that finds
//! transmitters across a band, FM/AM listening and an ADS-B (Mode S) decoder for a local
//! aircraft radar. The dongle itself is driven through `rtl_tcp` (see the app's radio.rs).

pub mod adsb;
pub mod burst;
pub mod dsp;
pub mod scan;

pub use rustfft::num_complex::Complex32;
