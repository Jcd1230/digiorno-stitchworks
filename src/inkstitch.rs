use crate::model::{Design, InputYAxis, StitchCommand, StitchPoint, Thread};
use anyhow::{anyhow, bail, Context, Result};
use serde::de::{Error as DeError, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use std::fmt;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// Coordinate scale into SHV tenths-of-a-millimeter-ish units. Default `1.0` matches our sample.
    pub scale: f64,
    /// Center the design extents around origin before writing SHV. Recommended.
    pub center: bool,
    /// Ink/Stitch JSON normally uses SVG/screen coordinates: +Y down.
    pub input_y_axis: InputYAxis,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            center: true,
            input_y_axis: InputYAxis::Down,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
struct InkStitchFile {
    #[serde(default)]
    threadlist: Vec<Thread>,
    #[serde(default)]
    stitches: Vec<RawStitch>,
    #[serde(default)]
    extras: Extras,
}

#[derive(Debug, Deserialize, Default)]
struct Extras {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Clone)]
struct RawStitch {
    x: f64,
    y: f64,
    command: String,
}

impl<'de> Deserialize<'de> for RawStitch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawStitchVisitor;

        impl<'de> Visitor<'de> for RawStitchVisitor {
            type Value = RawStitch;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an Ink/Stitch stitch row [x, y, command]")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let x: Value = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::custom("missing x value"))?;
                let y: Value = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::custom("missing y value"))?;
                let cmd: Value = seq
                    .next_element()?
                    .ok_or_else(|| A::Error::custom("missing command value"))?;

                let x = x
                    .as_f64()
                    .ok_or_else(|| A::Error::custom("x is not numeric"))?;
                let y = y
                    .as_f64()
                    .ok_or_else(|| A::Error::custom("y is not numeric"))?;
                let command = cmd
                    .as_str()
                    .ok_or_else(|| A::Error::custom("command is not a string"))?
                    .to_owned();

                Ok(RawStitch { x, y, command })
            }
        }

        deserializer.deserialize_seq(RawStitchVisitor)
    }
}

pub fn load_inkstitch_json_file(path: impl AsRef<Path>, options: &LoadOptions) -> Result<Design> {
    let path = path.as_ref();
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: InkStitchFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing Ink/Stitch JSON {}", path.display()))?;

    let fallback_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("design")
        .to_owned();

    normalize_inkstitch(parsed, fallback_name, options)
}

fn normalize_inkstitch(parsed: InkStitchFile, fallback_name: String, options: &LoadOptions) -> Result<Design> {
    if parsed.stitches.is_empty() {
        bail!("JSON contains no stitches");
    }
    if !options.scale.is_finite() || options.scale <= 0.0 {
        bail!("scale must be a finite positive number");
    }

    let coords: Vec<(f64, f64)> = parsed.stitches.iter().map(|s| (s.x, s.y)).collect();
    let (cx, cy) = if options.center {
        let min_x = coords.iter().map(|(x, _)| *x).fold(f64::INFINITY, f64::min);
        let max_x = coords.iter().map(|(x, _)| *x).fold(f64::NEG_INFINITY, f64::max);
        let min_y = coords.iter().map(|(_, y)| *y).fold(f64::INFINITY, f64::min);
        let max_y = coords.iter().map(|(_, y)| *y).fold(f64::NEG_INFINITY, f64::max);
        ((min_x + max_x) / 2.0, (min_y + max_y) / 2.0)
    } else {
        (0.0, 0.0)
    };

    let mut points = Vec::with_capacity(parsed.stitches.len());
    for raw in parsed.stitches {
        let x = ((raw.x - cx) * options.scale).round() as i32;
        let centered_y = (raw.y - cy) * options.scale;
        let y = match options.input_y_axis {
            InputYAxis::Down => (-centered_y).round() as i32,
            InputYAxis::Up => centered_y.round() as i32,
        };
        points.push(StitchPoint {
            x,
            y,
            command: StitchCommand::from_inkstitch_str(&raw.command),
        });
    }

    let mut threads = parsed.threadlist;
    if threads.is_empty() {
        threads.push(Thread {
            color: Some("#000000".to_owned()),
            description: Some("Black".to_owned()),
            ..Thread::default()
        });
    }

    let name = parsed
        .extras
        .name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback_name);

    if points.is_empty() {
        return Err(anyhow!("no usable stitch points were found"));
    }

    Ok(Design {
        name,
        threads,
        points,
    })
}
