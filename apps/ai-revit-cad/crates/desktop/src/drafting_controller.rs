//! Transient construction state. This module never owns or mutates a document.
use drafting_core::{Geometry, GeometryLimits, Point2, PolylineVertex};
use std::f64::consts::TAU;

const MAX_BULGE: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Line,
    Circle,
    Arc,
    Polyline,
    Move,
}

impl Tool {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "select" => Ok(Self::Select),
            "line" => Ok(Self::Line),
            "circle" => Ok(Self::Circle),
            "arc" => Ok(Self::Arc),
            "polyline" | "pline" => Ok(Self::Polyline),
            "move" => Ok(Self::Move),
            _ => Err(format!("Unknown tool: {value}")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Select => "SELECT",
            Self::Line => "LINE",
            Self::Circle => "CIRCLE",
            Self::Arc => "ARC",
            Self::Polyline => "PLINE",
            Self::Move => "MOVE",
        }
    }
}

#[derive(Clone, Debug)]
pub enum ControllerEffect {
    None,
    Create(Geometry),
    Move(Point2),
    SelectAt(Point2),
    Delete,
}

#[derive(Clone, Debug, Default)]
pub struct Controller {
    pub tool: Tool,
    pub points: Vec<Point2>,
    pub pointer: Option<Point2>,
    pub bulges: Vec<f64>,
    pub segment_bulge: f64,
    pub pending_geometry: Option<Geometry>,
    pub pending_move: Option<Point2>,
}

impl Controller {
    pub fn begin(&mut self, tool: Tool) {
        self.cancel();
        self.tool = tool;
    }

    pub fn cancel(&mut self) {
        self.points.clear();
        self.bulges.clear();
        self.clear_pointer();
        self.pending_geometry = None;
        self.pending_move = None;
        self.segment_bulge = 0.0;
    }

    /// Forget the transient cursor location without cancelling construction.
    pub fn clear_pointer(&mut self) {
        self.pointer = None;
    }

    pub fn is_pending(&self) -> bool {
        self.pending_geometry.is_some() || self.pending_move.is_some()
    }

    pub fn prompt(&self) -> String {
        if self.is_pending() {
            return format!(
                "{} — preview ready; Confirm / Enter, Escape cancels",
                self.tool.name()
            );
        }
        let step = match self.tool {
            Tool::Select => {
                "click an entity; repeated click cycles overlaps; ID input selects exactly"
            }
            Tool::Line if self.points.is_empty() => "first point — click or X Y (m)",
            Tool::Line => "second point — click or X Y (m)",
            Tool::Circle if self.points.is_empty() => "centre — click or X Y (m)",
            Tool::Circle => "radius — click rim or enter radius (m)",
            Tool::Arc if self.points.is_empty() => "centre — click or X Y (m)",
            Tool::Arc if self.points.len() == 1 => "start point — click, or radius start° sweep°",
            Tool::Arc => "end point — click for CCW arc, or signed sweep°",
            Tool::Polyline if self.points.is_empty() => {
                "first point — X Y; bulge belongs to outgoing segment"
            }
            Tool::Polyline => "next point — X Y [bulge]; Finish / Close, then Confirm",
            Tool::Move if self.points.is_empty() => {
                "base point — click, or DX DY (m) for selected entities"
            }
            Tool::Move => "target point — click or X Y (m)",
        };
        format!("{} — {step}", self.tool.name())
    }

    pub fn hover(&mut self, world: Point2) {
        self.pointer = Some(world);
    }

    pub fn click(&mut self, world: Point2) -> Result<ControllerEffect, String> {
        require_finite_point(world)?;
        require_bounded_point(world)?;
        if self.is_pending() {
            return Err("Preview is ready. Confirm it or cancel before adding points.".into());
        }
        match self.tool {
            Tool::Select => {
                self.pointer = Some(world);
                return Ok(ControllerEffect::SelectAt(world));
            }
            Tool::Line => {
                if let Some(start) = self.points.first().copied() {
                    let geometry = Geometry::Line { start, end: world };
                    validate_geometry(&geometry)?;
                    self.points.push(world);
                    self.pending_geometry = Some(geometry);
                } else {
                    self.points.push(world);
                }
            }
            Tool::Circle => {
                if let Some(centre) = self.points.first().copied() {
                    let geometry = Geometry::Circle {
                        center: centre,
                        radius: distance(centre, world),
                    };
                    self.set_pending_geometry(geometry)?;
                } else {
                    self.points.push(world);
                }
            }
            Tool::Arc => match self.points.len() {
                0 => self.points.push(world),
                1 => {
                    let center = self.points[0];
                    let radius = distance(center, world);
                    let limits = GeometryLimits::default();
                    if radius <= limits.linear_tolerance_m || radius > limits.max_abs_coordinate_m {
                        return Err(
                            "Arc start must be distinct from its centre and within the radius limit."
                                .into(),
                        );
                    }
                    self.points.push(world);
                }
                _ => {
                    let center = self.points[0];
                    let start = self.points[1];
                    if distance(center, world) <= GeometryLimits::default().linear_tolerance_m {
                        return Err("Arc end must be away from its centre.".into());
                    }
                    let start_angle = angle(center, start);
                    let sweep_angle = (angle(center, world) - start_angle).rem_euclid(TAU);
                    let geometry = Geometry::Arc {
                        center,
                        radius: distance(center, start),
                        start_angle,
                        sweep_angle,
                    };
                    self.set_pending_geometry(geometry)?;
                }
            },
            Tool::Polyline => {
                if self.points.len() >= GeometryLimits::default().max_vertices {
                    return Err("Interactive polyline budget exceeded (4,096 vertices).".into());
                }
                let bulge = if self.points.is_empty() {
                    self.segment_bulge
                } else {
                    let previous = *self.points.last().expect("non-empty points");
                    let previous_bulge = self.bulges.last().copied().unwrap_or(self.segment_bulge);
                    validate_geometry(&Geometry::Polyline {
                        vertices: vec![
                            PolylineVertex {
                                point: previous,
                                bulge: previous_bulge,
                            },
                            PolylineVertex {
                                point: world,
                                bulge: 0.0,
                            },
                        ],
                        closed: false,
                    })?;
                    0.0
                };
                self.points.push(world);
                self.bulges.push(bulge);
                self.segment_bulge = 0.0;
            }
            Tool::Move => {
                if let Some(base) = self.points.first().copied() {
                    let delta = Point2 {
                        x: world.x - base.x,
                        y: world.y - base.y,
                    };
                    require_bounded_move_delta(delta)?;
                    self.pending_move = Some(delta);
                } else {
                    self.points.push(world);
                }
            }
        }
        self.pointer = Some(world);
        Ok(ControllerEffect::None)
    }

    pub fn set_bulge(&mut self, bulge: f64) -> Result<(), String> {
        if self.is_pending() {
            return Err("Preview is ready. Confirm or cancel before changing bulges.".into());
        }
        validate_bulge(bulge)?;
        if let Some(last) = self.bulges.last_mut() {
            *last = bulge;
        }
        self.segment_bulge = bulge;
        Ok(())
    }

    pub fn finish_polyline(&mut self, closed: bool) -> Result<(), String> {
        if self.is_pending() {
            return Err("Preview is ready. Confirm it or cancel before finishing again.".into());
        }
        if self.tool != Tool::Polyline || self.points.len() < 2 {
            return Err("A polyline needs at least two points.".into());
        }
        let geometry = Geometry::Polyline {
            vertices: self
                .points
                .iter()
                .copied()
                .zip(self.bulges.iter().copied())
                .map(|(point, bulge)| PolylineVertex { point, bulge })
                .collect(),
            closed,
        };
        validate_geometry(&geometry)?;
        self.pending_geometry = Some(geometry);
        Ok(())
    }

    pub fn confirm(&mut self) -> Result<ControllerEffect, String> {
        if let Some(geometry) = self.pending_geometry.as_ref() {
            validate_geometry(geometry)?;
            let geometry = self
                .pending_geometry
                .take()
                .expect("checked pending geometry");
            self.cancel();
            return Ok(ControllerEffect::Create(geometry));
        }
        if let Some(delta) = self.pending_move {
            require_bounded_move_delta(delta)?;
            self.pending_move = None;
            self.cancel();
            return Ok(ControllerEffect::Move(delta));
        }
        Err("No complete preview to confirm.".into())
    }

    /// Numeric input advances precisely the same construction state as a click.
    pub fn numeric(&mut self, input: &str) -> Result<ControllerEffect, String> {
        if input.trim().is_empty() && self.is_pending() {
            return self.confirm();
        }
        let values = parse_numbers(input)?;
        if self.is_pending() {
            return Err("Confirm or cancel the ready preview first.".into());
        }
        match self.tool {
            Tool::Circle if self.points.len() == 1 && values.len() == 1 => {
                let geometry = Geometry::Circle {
                    center: self.points[0],
                    radius: values[0],
                };
                self.set_pending_geometry(geometry)?;
            }
            Tool::Arc if self.points.len() == 1 && values.len() == 3 => {
                let geometry = Geometry::Arc {
                    center: self.points[0],
                    radius: values[0],
                    start_angle: values[1].to_radians(),
                    sweep_angle: values[2].to_radians(),
                };
                self.set_pending_geometry(geometry)?;
            }
            Tool::Arc if self.points.len() == 2 && values.len() == 1 => {
                let center = self.points[0];
                let start = self.points[1];
                let geometry = Geometry::Arc {
                    center,
                    radius: distance(center, start),
                    start_angle: angle(center, start),
                    sweep_angle: values[0].to_radians(),
                };
                self.set_pending_geometry(geometry)?;
            }
            Tool::Move if self.points.is_empty() && values.len() == 2 => {
                let delta = Point2 {
                    x: values[0],
                    y: values[1],
                };
                require_bounded_move_delta(delta)?;
                self.pending_move = Some(delta);
            }
            Tool::Polyline if values.len() == 3 => {
                validate_bulge(values[2])?;
                self.click(Point2 {
                    x: values[0],
                    y: values[1],
                })?;
                self.set_bulge(values[2])?;
            }
            _ if values.len() == 2 => {
                return self.click(Point2 {
                    x: values[0],
                    y: values[1],
                });
            }
            _ => {
                return Err(
                    "Expected X Y, radius, or radius start° sweep° for the current step.".into(),
                );
            }
        }
        Ok(ControllerEffect::None)
    }

    pub fn preview(&self) -> Vec<Geometry> {
        if let Some(geometry) = &self.pending_geometry {
            return valid_preview(geometry.clone());
        }
        let Some(pointer) = self.pointer else {
            return Vec::new();
        };
        let Some(first) = self.points.first().copied() else {
            return Vec::new();
        };
        match self.tool {
            Tool::Line => valid_preview(Geometry::Line {
                start: first,
                end: pointer,
            }),
            Tool::Circle => valid_preview(Geometry::Circle {
                center: first,
                radius: distance(first, pointer),
            }),
            Tool::Arc if self.points.len() == 1 => valid_preview(Geometry::Circle {
                center: first,
                radius: distance(first, pointer),
            }),
            Tool::Arc => {
                let start = self.points[1];
                let start_angle = angle(first, start);
                valid_preview(Geometry::Arc {
                    center: first,
                    radius: distance(first, start),
                    start_angle,
                    sweep_angle: (angle(first, pointer) - start_angle).rem_euclid(TAU),
                })
            }
            Tool::Polyline => {
                if self.points.len() >= GeometryLimits::default().max_vertices {
                    return Vec::new();
                }
                let mut vertices: Vec<PolylineVertex> = self
                    .points
                    .iter()
                    .copied()
                    .zip(self.bulges.iter().copied())
                    .map(|(point, bulge)| PolylineVertex { point, bulge })
                    .collect();
                vertices.push(PolylineVertex {
                    point: pointer,
                    bulge: 0.0,
                });
                valid_preview(Geometry::Polyline {
                    vertices,
                    closed: false,
                })
            }
            _ => Vec::new(),
        }
    }

    fn set_pending_geometry(&mut self, geometry: Geometry) -> Result<(), String> {
        validate_geometry(&geometry)?;
        self.pending_geometry = Some(geometry);
        Ok(())
    }
}

fn parse_numbers(input: &str) -> Result<Vec<f64>, String> {
    let mut values = Vec::new();
    for word in input
        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
        .filter(|s| !s.is_empty())
    {
        if values.len() == 4 {
            return Err("Too many numeric parameters.".into());
        }
        let value: f64 = word
            .parse()
            .map_err(|_| format!("Invalid number: {word}"))?;
        if !value.is_finite() {
            return Err("Numeric parameters must be finite.".into());
        }
        values.push(value);
    }
    if values.is_empty() {
        return Err("Enter numeric parameters.".into());
    }
    Ok(values)
}

fn require_finite_point(point: Point2) -> Result<(), String> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err("Coordinates must be finite.".into())
    }
}
fn require_bounded_point(point: Point2) -> Result<(), String> {
    let max = GeometryLimits::default().max_abs_coordinate_m;
    if point.x.abs() > max || point.y.abs() > max {
        Err("Coordinates exceed the bounded geometry domain (±1e12 m).".into())
    } else {
        Ok(())
    }
}
fn require_bounded_move_delta(delta: Point2) -> Result<(), String> {
    let max = GeometryLimits::default().max_abs_coordinate_m * 2.0;
    if !delta.is_finite() {
        Err("Move delta must be finite.".into())
    } else if delta.x.abs() > max || delta.y.abs() > max {
        Err("Move delta exceeds the bounded geometry domain.".into())
    } else {
        Ok(())
    }
}
fn validate_geometry(geometry: &Geometry) -> Result<(), String> {
    geometry
        .validate(GeometryLimits::default())
        .map_err(|error| error.to_string())
}
fn valid_preview(geometry: Geometry) -> Vec<Geometry> {
    if validate_geometry(&geometry).is_ok() {
        vec![geometry]
    } else {
        Vec::new()
    }
}
fn validate_bulge(bulge: f64) -> Result<(), String> {
    if !bulge.is_finite() || bulge.abs() > MAX_BULGE {
        Err("Bulge must be finite and within ±1,000,000.".into())
    } else {
        Ok(())
    }
}
fn distance(a: Point2, b: Point2) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
fn angle(center: Point2, point: Point2) -> f64 {
    (point.y - center.y).atan2(point.x - center.x)
}
