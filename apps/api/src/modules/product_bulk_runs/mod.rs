//! Durable, revisioned product import runs; parser/AI output is always staged for review.
pub mod claude;
pub mod dto;
pub mod extraction;
pub mod handlers;
pub mod parser;
pub mod routes;
pub mod service;
pub mod spend;
pub mod worker;
pub use dto::{ExtractionStep, RawRow, Source};

#[cfg(test)]
mod parser_tests;
