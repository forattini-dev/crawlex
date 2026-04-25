use serde::{Deserialize, Serialize};

/// A canned "this is Chrome N on Linux" profile.
///
/// The goal is **coherence**: every header, JS shim property, and Chrome
/// launch flag that references a version number points to the same number.
/// Detectors cross-check UA, `userAgentData`, `sec-ch-ua` and GL strings;
/// a mismatch lights up bot trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    Chrome131Stable,
    Chrome132Stable,
    Chrome149Stable,
}

impl Profile {
    pub fn user_agent(self) -> &'static str {
        match self {
            Profile::Chrome131Stable => {
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
            }
            Profile::Chrome132Stable => {
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36"
            }
            Profile::Chrome149Stable => {
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/149.0.0.0 Safari/537.36"
            }
        }
    }

    pub fn sec_ch_ua(self) -> &'static str {
        match self {
            Profile::Chrome131Stable => {
                "\"Google Chrome\";v=\"131\", \"Chromium\";v=\"131\", \"Not_A Brand\";v=\"24\""
            }
            Profile::Chrome132Stable => {
                "\"Google Chrome\";v=\"132\", \"Chromium\";v=\"132\", \"Not_A Brand\";v=\"24\""
            }
            Profile::Chrome149Stable => {
                "\"Google Chrome\";v=\"149\", \"Chromium\";v=\"149\", \"Not_A Brand\";v=\"24\""
            }
        }
    }

    /// Major version (integer) exposed for cross-checks — render pool uses
    /// this to verify the Chrome binary on disk matches our claimed profile.
    pub fn major_version(self) -> u32 {
        match self {
            Profile::Chrome131Stable => 131,
            Profile::Chrome132Stable => 132,
            Profile::Chrome149Stable => 149,
        }
    }

    /// Full "131.0.6778.85"-style version string used in the userAgentData
    /// high-entropy hints. Minor/patch numbers don't matter for header
    /// fingerprinting so we pin stable placeholder values per major.
    pub fn ua_full_version(self) -> &'static str {
        match self {
            Profile::Chrome131Stable => "131.0.6778.85",
            Profile::Chrome132Stable => "132.0.6834.83",
            Profile::Chrome149Stable => "149.0.7712.82",
        }
    }

    /// JSON list used as `navigator.userAgentData.brands`. Must agree with
    /// `sec_ch_ua()` above — we generate it the same way both places.
    pub fn ua_brands_json(self) -> String {
        let v = self.major_version();
        format!(
            r#"[{{"brand":"Google Chrome","version":"{v}"}},{{"brand":"Chromium","version":"{v}"}},{{"brand":"Not_A Brand","version":"24"}}]"#
        )
    }

    pub fn fullversion_brands_json(self) -> String {
        let full = self.ua_full_version();
        format!(
            r#"[{{"brand":"Google Chrome","version":"{full}"}},{{"brand":"Chromium","version":"{full}"}},{{"brand":"Not_A Brand","version":"24.0.0.0"}}]"#
        )
    }

    /// Pick the closest profile for a real Chrome major version detected on
    /// disk. Returns the exact match if we know it, otherwise the newest
    /// profile that isn't newer than the runtime binary.
    pub fn from_detected_major(major: u32) -> Profile {
        match major {
            131 => Profile::Chrome131Stable,
            132 => Profile::Chrome132Stable,
            149 => Profile::Chrome149Stable,
            n if n >= 149 => Profile::Chrome149Stable,
            n if n >= 132 => Profile::Chrome132Stable,
            _ => Profile::Chrome131Stable,
        }
    }
}
