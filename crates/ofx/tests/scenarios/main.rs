//! Scenarios that load the test OpenFX plug-in (`examples/ofx/ec-test-ofx`) into the host: the scan
//! cache, render crashes, re-entrant renders, instance and sequence management and shutdown. They
//! are one test binary (its debug information is linked once), in one module each. Scenarios that
//! need a start-up of their own run it as a child process of this binary (`common::Fixture`) and
//! name the child test by its full path (`scan_cache::child_start`).
//!
//! Set `EC_TEST_OFX_BUNDLE` to the built `ec-test-ofx.ofx.bundle`; without it the scenarios return.

#[path = "../common/mod.rs"]
mod common;
mod instances;
mod reentrancy;
mod render_crash;
mod scan_cache;
