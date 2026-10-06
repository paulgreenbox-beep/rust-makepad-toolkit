#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

impl LogicalPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ViewportPoint {
    pub x: f64,
    pub y: f64,
}

impl ViewportPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenPoint {
    pub x: f64,
    pub y: f64,
}

impl ScreenPoint {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalSize {
    pub width: f64,
    pub height: f64,
}

impl LogicalSize {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn non_negative(self) -> Self {
        Self {
            width: self.width.max(0.0),
            height: self.height.max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenSize {
    pub width: f64,
    pub height: f64,
}

impl ScreenSize {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    pub fn non_negative(self) -> Self {
        Self {
            width: self.width.max(0.0),
            height: self.height.max(0.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogicalRect {
    pub origin: LogicalPoint,
    pub size: LogicalSize,
}

impl LogicalRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: LogicalPoint::new(x, y),
            size: LogicalSize::new(width, height),
        }
    }

    pub fn right(self) -> f64 {
        self.origin.x + self.size.width
    }

    pub fn bottom(self) -> f64 {
        self.origin.y + self.size.height
    }

    pub fn is_valid(self) -> bool {
        self.origin.x.is_finite()
            && self.origin.y.is_finite()
            && self.size.width.is_finite()
            && self.size.height.is_finite()
            && self.size.width > 0.0
            && self.size.height > 0.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && self.origin.x < other.right()
            && other.origin.x < self.right()
            && self.origin.y < other.bottom()
            && other.origin.y < self.bottom()
    }

    pub fn inset(self, left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self::new(
            self.origin.x + left,
            self.origin.y + top,
            (self.size.width - left - right).max(0.0),
            (self.size.height - top - bottom).max(0.0),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScreenRect {
    pub origin: ScreenPoint,
    pub size: ScreenSize,
}

impl ScreenRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            origin: ScreenPoint::new(x, y),
            size: ScreenSize::new(width, height),
        }
    }

    pub fn right(self) -> f64 {
        self.origin.x + self.size.width
    }

    pub fn bottom(self) -> f64 {
        self.origin.y + self.size.height
    }

    pub fn is_valid(self) -> bool {
        self.origin.x.is_finite()
            && self.origin.y.is_finite()
            && self.size.width.is_finite()
            && self.size.height.is_finite()
            && self.size.width > 0.0
            && self.size.height > 0.0
    }

    pub fn intersects(self, other: Self) -> bool {
        self.is_valid()
            && other.is_valid()
            && self.origin.x < other.right()
            && other.origin.x < self.right()
            && self.origin.y < other.bottom()
            && other.origin.y < self.bottom()
    }

    pub fn intersection(self, other: Self) -> Self {
        let x = self.origin.x.max(other.origin.x);
        let y = self.origin.y.max(other.origin.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= x || bottom <= y {
            return Self::new(x, y, 0.0, 0.0);
        }
        Self::new(x, y, right - x, bottom - y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_rejects_disjoint_rects() {
        let left = ScreenRect::new(0.0, 0.0, 10.0, 10.0);
        let right = ScreenRect::new(20.0, 20.0, 5.0, 5.0);
        assert!(!left.intersects(right));
        assert_eq!(left.intersection(right).size, ScreenSize::new(0.0, 0.0));
    }

    #[test]
    fn logical_inset_is_page_local() {
        let page = LogicalRect::new(10.0, 20.0, 200.0, 300.0);
        assert_eq!(
            page.inset(30.0, 40.0, 50.0, 60.0),
            LogicalRect::new(40.0, 60.0, 120.0, 200.0)
        );
    }
}
