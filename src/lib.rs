//! acreage: a live, zoomable treemap disk usage browser for Linux.
//!
//! `model`, `scan`, `filter`, `layout` and `view` have no UI dependency, so they can be
//! tested on their own and reused by a headless report mode.

pub mod actions;
pub mod app;
pub mod config;
pub mod filter;
pub mod layout;
pub mod model;
pub mod report;
pub mod scan;
pub mod screens;
pub mod testhook;
pub mod ui;
pub mod view;
pub mod watch;
