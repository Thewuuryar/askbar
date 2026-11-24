//! `egress-proxy` (ADR 0056): the one way out of the stack. Default deny, one allowlist per source.
//!
//! - **[`policy`]** is the pure part: the host check, the address classes and the verdict. No I/O, no clock, no panic on any input.

#![forbid(unsafe_code)]

pub mod policy;
