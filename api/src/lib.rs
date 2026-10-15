//! Shared helpers and HTTP handlers for the lamp API.
//!
//! The Axum server bootstrap lives in the binary target (`src/main.rs`);
//! anything reusable (and unit-testable on its own) belongs here: request
//! handlers with their DTOs (`admin`, `catalogue`, `content`, `orders`),
//! shared HTTP plumbing
//! (`state`), env configuration knobs (`config`) and pure helpers
//! (`money`, `slug`).

pub mod admin;
pub mod admin_crud;
pub mod catalogue;
pub mod config;
pub mod content;
pub mod money;
pub mod orders;
pub mod slug;
pub mod state;
