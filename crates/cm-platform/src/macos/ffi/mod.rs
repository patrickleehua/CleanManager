//! Minimal raw FFI to the Apple frameworks the macOS backends need, written against the SDK
//! headers (each declaration cites its header signature) instead of binding crates:
//!
//! - [`cf`]: CoreFoundation strings, URLs, data, dictionaries, property lists and preferences,
//!   converted to and from a small Rust [`cf::Plist`] value.
//! - [`objc`]: the Objective-C runtime (`objc_msgSend` with exact function types), `NSString` and
//!   `NSURL` helpers and an autorelease pool guard; links Foundation and AppKit.
//! - [`security`]: static code signing facts (Team ID, signing authorities, Apple anchor).
//! - [`iokit`]: one-service registry property reads.
//!
//! Every owned CoreFoundation / Objective-C object is held by an RAII wrapper that releases it
//! exactly once; nothing here retains an object past the wrapper's lifetime.

// Foundations for the later macOS work packages: most consumers land with them.
#![allow(dead_code)]

pub(crate) mod cf;
pub(crate) mod iokit;
pub(crate) mod objc;
pub(crate) mod security;
