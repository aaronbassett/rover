//! Peek `<base href>` from raw HTML before readabilityrs touches it.

use url::Url;

pub fn read_base_href(_html: &str) -> Option<Url> {
    None
}
