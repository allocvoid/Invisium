//! Invisium: a from-scratch browser engine.
//!
//! Everything that makes a browser engine an engine is implemented in this workspace: the HTML
//! parser, the CSS parser and cascade, layout, painting (this crate), the HTTP stack and privacy
//! layer (`invisium-net`), the JavaScript engine (`invisium-js`) and the GPU renderer
//! (`invisium-gfx`). Third-party crates are used only for platform plumbing: the GPU API, TLS
//! cryptography, image codecs, font outlines and the Tor protocol.

pub mod atom;
pub mod css;
pub mod dom;
pub mod html;
pub mod layout;
pub mod page;
pub mod paint;
pub mod script;
pub mod style;
pub mod text;

pub use invisium_draw as draw;
