//! Edge-case tests of the Microsoft 365 side: Graph client, calendar, tasks/
//! snooze/recurrence, compose and CRUD of contacts/categories/signatures/
//! lanes – against the fake Graph server and small scripted servers.
//!
//! One test binary (instead of one per file) keeps build times and disk use
//! down.

mod calendar;
mod client;
mod common;
mod compose;
mod crud;
mod tasks;
