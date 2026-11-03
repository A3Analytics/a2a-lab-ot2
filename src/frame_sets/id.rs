//! Validated frame-set identifiers.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::FrameSetError;

const MAX_LEN: usize = 128;

fn validate(value: &str) -> Result<(), FrameSetError> {
    let allowed = value.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | ':' | '-')
    });
    if value.is_empty() || value.len() > MAX_LEN || !allowed {
        return Err(FrameSetError::invalid(
            "id",
            format!("must be 1..={MAX_LEN} ASCII letters, digits, or . _ : -"),
        ));
    }
    Ok(())
}

macro_rules! id_type {
    ($name:ident, $label:literal) => {
        #[doc = concat!("Identifier for a ", $label, ".")]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            /// Validates and creates an identifier.
            pub fn new(value: impl AsRef<str>) -> Result<Self, FrameSetError> {
                let value = value.as_ref();
                validate(value)?;
                Ok(Self(value.to_owned()))
            }

            /// Returns the identifier text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }
    };
}

id_type!(FrameSetSourceId, "frame-set source");
id_type!(FrameChannelId, "frame channel");
id_type!(FrameSetId, "frame set");
