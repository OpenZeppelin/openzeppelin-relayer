//! # API Module
//!
//! Contains HTTP API implementation for the relayer service.
//!
//! ## Structure
//!
//! * `controllers` - Request handling and business logic
//! * `routes` - API endpoint definitions and routing
//! * `middleware` - HTTP middleware for timeouts, concurrency, etc.

pub mod controllers;

pub mod middleware;

pub mod routes;
