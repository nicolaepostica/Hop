//! Keysym → keycode lookup built from the server's keyboard mapping.

use std::collections::HashMap;

use hop_platform::PlatformError;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::ConnectionExt as _;

/// Maps X11 keysyms (what the protocol sends on the wire) to keycodes
/// (what `XTest` wants). Built once per `X11Screen`; regenerated on
/// `MappingNotify` is a future concern — the first-user use case (US
/// QWERTY layout) keeps the same mapping for the lifetime of a session.
#[derive(Debug)]
pub struct KeyMap {
    keysym_to_keycode: HashMap<u32, u8>,
}

impl KeyMap {
    /// Query the server and build the map.
    pub fn load<C: Connection>(conn: &C) -> Result<Self, PlatformError> {
        let setup = conn.setup();
        let min_keycode = setup.min_keycode;
        let max_keycode = setup.max_keycode;
        let count = max_keycode - min_keycode + 1;

        let reply = conn
            .get_keyboard_mapping(min_keycode, count)
            .map_err(PlatformError::connection_lost)?
            .reply()
            .map_err(PlatformError::connection_lost)?;

        let keysyms_per_keycode = reply.keysyms_per_keycode as usize;
        let mut keysym_to_keycode = HashMap::new();

        for (index, chunk) in reply.keysyms.chunks(keysyms_per_keycode).enumerate() {
            // Insert only the first non-zero keysym per keycode so we
            // inject an unshifted press by default; callers supply the
            // Shift modifier explicitly via the `mods` argument to
            // `inject_key`.
            if let Some(&keysym) = chunk.iter().find(|&&k| k != 0) {
                keysym_to_keycode
                    .entry(keysym)
                    .or_insert_with(|| min_keycode + u8::try_from(index).unwrap_or(0));
            }
        }

        Ok(Self { keysym_to_keycode })
    }

    /// Look up a keycode for an X11 keysym. `None` means the current
    /// layout has no keycode for that symbol.
    #[must_use]
    pub fn keycode(&self, keysym: u32) -> Option<u8> {
        self.keysym_to_keycode.get(&keysym).copied()
    }
}
