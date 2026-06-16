use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputYAxis {
    /// SVG/screen coordinates: +Y moves down the page/screen. This is the normal Ink/Stitch JSON export convention.
    Down,
    /// Cartesian coordinates: +Y moves up.
    Up,
}

impl Default for InputYAxis {
    fn default() -> Self {
        Self::Down
    }
}

impl fmt::Display for InputYAxis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Down => write!(f, "down"),
            Self::Up => write!(f, "up"),
        }
    }
}

impl FromStr for InputYAxis {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "down" | "svg" | "screen" => Ok(Self::Down),
            "up" | "cartesian" => Ok(Self::Up),
            other => Err(format!("unknown Y axis mode {other:?}; use 'down' or 'up'")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignatureMode {
    /// Write the 86-byte official Viking notice observed in public notes.
    Official,
    /// Write 86 zero bytes, matching the Embird-generated samples we inspected.
    Zero,
}

impl Default for SignatureMode {
    fn default() -> Self {
        Self::Official
    }
}

impl fmt::Display for SignatureMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Official => write!(f, "official"),
            Self::Zero => write!(f, "zero"),
        }
    }
}

impl FromStr for SignatureMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "official" | "viking" => Ok(Self::Official),
            "zero" | "zeros" | "embird" => Ok(Self::Zero),
            other => Err(format!(
                "unknown signature mode {other:?}; use 'official' or 'zero'"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Thread {
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub catalog_number: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StitchCommand {
    Stitch,
    Jump,
    Trim,
    ColorChange,
    Stop,
    End,
    Other(String),
}

impl StitchCommand {
    pub fn from_inkstitch_str(value: &str) -> Self {
        let normalized = value.trim().to_ascii_uppercase();
        let command = normalized.split_whitespace().next().unwrap_or("");
        match command {
            "STITCH" => Self::Stitch,
            "JUMP" => Self::Jump,
            "TRIM" => Self::Trim,
            "COLOR" | "COLOR_CHANGE" | "COLORCHANGE" => Self::ColorChange,
            "STOP" => Self::Stop,
            "END" => Self::End,
            _ => Self::Other(normalized),
        }
    }

    pub fn is_drawn_stitch(&self) -> bool {
        matches!(self, Self::Stitch)
    }

    pub fn is_positioning_move(&self) -> bool {
        matches!(self, Self::Stitch | Self::Jump | Self::Trim)
    }
}

impl fmt::Display for StitchCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stitch => write!(f, "STITCH"),
            Self::Jump => write!(f, "JUMP"),
            Self::Trim => write!(f, "TRIM"),
            Self::ColorChange => write!(f, "COLOR_CHANGE"),
            Self::Stop => write!(f, "STOP"),
            Self::End => write!(f, "END"),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

/// A normalized point in centered Cartesian 0.1 mm-ish units: +X right, +Y up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StitchPoint {
    pub x: i32,
    pub y: i32,
    pub command: StitchCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Design {
    pub name: String,
    pub threads: Vec<Thread>,
    pub points: Vec<StitchPoint>,
}

impl Design {
    pub fn stats(&self) -> DesignStats {
        let mut min_x = i32::MAX;
        let mut max_x = i32::MIN;
        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut stitches = 0usize;
        let mut jumps = 0usize;
        let mut trims = 0usize;
        let mut color_changes = 0usize;
        let mut ends = 0usize;

        for point in &self.points {
            if point.command.is_positioning_move() {
                min_x = min_x.min(point.x);
                max_x = max_x.max(point.x);
                min_y = min_y.min(point.y);
                max_y = max_y.max(point.y);
            }
            match &point.command {
                StitchCommand::Stitch => stitches += 1,
                StitchCommand::Jump => jumps += 1,
                StitchCommand::Trim => trims += 1,
                StitchCommand::ColorChange | StitchCommand::Stop => color_changes += 1,
                StitchCommand::End => ends += 1,
                StitchCommand::Other(_) => {}
            }
        }

        if min_x == i32::MAX {
            min_x = 0;
            max_x = 0;
            min_y = 0;
            max_y = 0;
        }

        DesignStats {
            point_count: self.points.len(),
            thread_count: self.threads.len(),
            stitches,
            jumps,
            trims,
            color_changes,
            ends,
            left: min_x,
            right: max_x,
            bottom: min_y,
            top: max_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignStats {
    pub point_count: usize,
    pub thread_count: usize,
    pub stitches: usize,
    pub jumps: usize,
    pub trims: usize,
    pub color_changes: usize,
    pub ends: usize,
    pub left: i32,
    pub right: i32,
    pub bottom: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

#[cfg(test)]
mod tests {
    use super::StitchCommand;

    #[test]
    fn parses_inkstitch_color_change_with_metadata() {
        assert_eq!(
            StitchCommand::from_inkstitch_str("COLOR_CHANGE t1 n2"),
            StitchCommand::ColorChange
        );
    }
}
