//! Pure, semantic-first spatial interaction contracts.

use serde::{Deserialize, Serialize};
use toolkit_windows_automation::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelativePoint {
    pub x: f64,
    pub y: f64,
}

impl RelativePoint {
    pub fn validate(self) -> Result<Self, SpatialError> {
        if self.x.is_finite()
            && self.y.is_finite()
            && (0.0..=1.0).contains(&self.x)
            && (0.0..=1.0).contains(&self.y)
        {
            Ok(self)
        } else {
            Err(SpatialError::InvalidCoordinate {
                x: self.x,
                y: self.y,
            })
        }
    }
    pub fn resolve(self, bounds: Rect) -> Result<(i32, i32), SpatialError> {
        self.validate()?;
        if !bounds.is_actionable() {
            return Err(SpatialError::InvalidBounds(bounds));
        }
        Ok((
            bounds.x + (self.x * f64::from(bounds.width - 1)).round() as i32,
            bounds.y + (self.y * f64::from(bounds.height - 1)).round() as i32,
        ))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelativeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RelativeRect {
    pub fn validate(self) -> Result<Self, SpatialError> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.width > 0.0
            && self.height > 0.0
            && self.x + self.width <= 1.0
            && self.y + self.height <= 1.0
        {
            Ok(self)
        } else {
            Err(SpatialError::InvalidRegion(self))
        }
    }
    pub fn resolve(self, bounds: Rect) -> Result<Rect, SpatialError> {
        self.validate()?;
        if !bounds.is_actionable() {
            return Err(SpatialError::InvalidBounds(bounds));
        }
        Ok(Rect {
            x: bounds.x + (self.x * f64::from(bounds.width)).round() as i32,
            y: bounds.y + (self.y * f64::from(bounds.height)).round() as i32,
            width: (self.width * f64::from(bounds.width)).round().max(1.0) as i32,
            height: (self.height * f64::from(bounds.height)).round().max(1.0) as i32,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Modifier {
    CTRL,
    SHIFT,
    ALT,
}

impl Modifier {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => Some(Self::CTRL),
            "SHIFT" => Some(Self::SHIFT),
            "ALT" | "MENU" => Some(Self::ALT),
            _ => None,
        }
    }
}

pub fn parse_modifiers(value: Option<&serde_json::Value>) -> Result<Vec<Modifier>, SpatialError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Some(values) = value.as_array() else {
        return Err(SpatialError::InvalidModifiers);
    };
    let mut result = Vec::new();
    for value in values {
        let Some(name) = value.as_str() else {
            return Err(SpatialError::InvalidModifiers);
        };
        let Some(modifier) = Modifier::parse(name) else {
            return Err(SpatialError::InvalidModifiers);
        };
        if !result.contains(&modifier) {
            result.push(modifier);
        }
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    ElementRelative,
    ElementPixels,
    WindowClient,
    WindowPixels,
    ScreenPixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DpiMetadata {
    pub dpi: u32,
    pub scale_factor: f64,
    pub coordinate_space: CoordinateSpace,
}

impl Default for DpiMetadata {
    fn default() -> Self {
        Self {
            dpi: 96,
            scale_factor: 1.0,
            coordinate_space: CoordinateSpace::ScreenPixels,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SpatialError {
    InvalidCoordinate { x: f64, y: f64 },
    InvalidBounds(Rect),
    InvalidRegion(RelativeRect),
    InvalidModifiers,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPoint {
    pub relative: RelativePoint,
    pub element_bounds: Rect,
    pub screen: (i32, i32),
    pub dpi: DpiMetadata,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bounds() -> Rect {
        Rect {
            x: 100,
            y: 200,
            width: 401,
            height: 201,
        }
    }
    #[test]
    fn relative_points_resolve_to_element_edges() {
        assert_eq!(
            RelativePoint { x: 0.0, y: 0.0 }.resolve(bounds()).unwrap(),
            (100, 200)
        );
        assert_eq!(
            RelativePoint { x: 1.0, y: 1.0 }.resolve(bounds()).unwrap(),
            (500, 400)
        );
        assert_eq!(
            RelativePoint { x: 0.5, y: 0.5 }.resolve(bounds()).unwrap(),
            (300, 300)
        );
    }
    #[test]
    fn invalid_coordinates_and_bounds_are_rejected() {
        assert!(matches!(
            RelativePoint { x: 1.1, y: 0.5 }.validate(),
            Err(SpatialError::InvalidCoordinate { .. })
        ));
        assert!(matches!(
            RelativeRect {
                x: 0.8,
                y: 0.0,
                width: 0.3,
                height: 0.2
            }
            .validate(),
            Err(SpatialError::InvalidRegion(_))
        ));
        assert!(matches!(
            RelativePoint { x: 0.5, y: 0.5 }.resolve(Rect::default()),
            Err(SpatialError::InvalidBounds(_))
        ));
    }
    #[test]
    fn region_resolution_is_pixel_stable() {
        assert_eq!(
            RelativeRect {
                x: 0.25,
                y: 0.25,
                width: 0.5,
                height: 0.5
            }
            .resolve(bounds())
            .unwrap(),
            Rect {
                x: 200,
                y: 250,
                width: 201,
                height: 101
            }
        );
    }
}
