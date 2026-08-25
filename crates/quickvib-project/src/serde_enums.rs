//! serde adapters for the `quickvib-core` enumerations.
//!
//! `quickvib-core` deliberately has no `serde` dependency (D18 keeps serde confined to this
//! crate), so the schema fields use `deserialize_with`/`serialize_with` and go through the
//! enums' own `FromStr`/`Display`.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, Unexpected, Visitor};
use serde::Serializer;

use quickvib_core::{BackendKind, ExportFormat, SampleUnit};

struct StrVisitor {
    expecting: &'static str,
}

impl Visitor<'_> for StrVisitor {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(v.to_owned())
    }
}

fn parse_with<'de, D, T>(
    de: D,
    expecting: &'static str,
    allowed: &'static str,
) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
{
    let raw = de.deserialize_str(StrVisitor { expecting })?;
    T::from_str(&raw).map_err(|_| de::Error::invalid_value(Unexpected::Str(&raw), &allowed))
}

pub(crate) fn de_unit<'de, D: Deserializer<'de>>(de: D) -> Result<SampleUnit, D::Error> {
    parse_with(
        de,
        "a sample unit",
        "velocity_um_s, displacement_um or acceleration_m_s2",
    )
}

pub(crate) fn ser_unit<S: Serializer>(v: &SampleUnit, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(v.as_schema_str())
}

pub(crate) fn de_format<'de, D: Deserializer<'de>>(de: D) -> Result<ExportFormat, D::Error> {
    parse_with(de, "an export format", "CSV or TXT")
}

pub(crate) fn ser_format<S: Serializer>(v: &ExportFormat, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(v.as_scpi_str())
}

pub(crate) fn de_backend<'de, D: Deserializer<'de>>(de: D) -> Result<BackendKind, D::Error> {
    parse_with(de, "a backend name", "mock or m300")
}

pub(crate) fn ser_backend<S: Serializer>(v: &BackendKind, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(v.as_str())
}
