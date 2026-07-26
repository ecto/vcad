//! Named datum geometry — planes, axes, and points that geometry is placed
//! *relative to* instead of at literal coordinates.
//!
//! A datum is reference geometry with a name and no mass. Its coordinates are
//! [`Expr`]s, so a datum resolves through the same parameter environment as
//! [`Bindings`](crate::Bindings): a plane declared at `femur_inner` is the
//! parameter `femur_inner`, and every part that references that plane moves
//! together when the parameter moves.
//!
//! Why this exists: the failure mode it removes is two parts each carrying
//! their own literal for what is physically *one* plane, and silently
//! disagreeing about where it is. A datum makes the plane a single named
//! entity — parts reference it, they cannot each hold a private copy of it,
//! and the packing stack becomes machine-readable rather than a comment.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::parameters::{Expr, ResolveError};

/// A principal axis, used by the axis-aligned datum-plane shorthand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum PrincipalAxis {
    /// The X axis.
    X,
    /// The Y axis.
    Y,
    /// The Z axis.
    Z,
}

impl PrincipalAxis {
    /// Unit vector along this axis.
    pub fn unit(self) -> [f64; 3] {
        match self {
            Self::X => [1.0, 0.0, 0.0],
            Self::Y => [0.0, 1.0, 0.0],
            Self::Z => [0.0, 0.0, 1.0],
        }
    }

    /// Index of this axis in an `[x, y, z]` triple.
    pub fn index(self) -> usize {
        match self {
            Self::X => 0,
            Self::Y => 1,
            Self::Z => 2,
        }
    }

    /// Parse from a single-letter name (`"x"`, `"Y"`, …).
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "x" | "X" => Some(Self::X),
            "y" | "Y" => Some(Self::Y),
            "z" | "Z" => Some(Self::Z),
            _ => None,
        }
    }
}

/// Reference geometry: a named plane, axis, or point.
///
/// Every coordinate is an [`Expr`], so datums participate in the parameter
/// DAG. An axis-aligned lane plane is the common case and has a constructor
/// ([`Datum::axis_plane`]) that keeps the offset symbolic while pinning the
/// normal to a literal unit vector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum Datum {
    /// An infinite plane through `origin` with the given `normal`.
    Plane {
        /// A point on the plane.
        origin: [Expr; 3],
        /// Plane normal (need not be unit length).
        normal: [Expr; 3],
    },
    /// An infinite line through `origin` along `direction`.
    Axis {
        /// A point on the axis.
        origin: [Expr; 3],
        /// Axis direction (need not be unit length).
        direction: [Expr; 3],
    },
    /// A single named point.
    Point {
        /// The point's position.
        position: [Expr; 3],
    },
}

/// Zero-valued coordinate triple.
fn zeros() -> [Expr; 3] {
    [Expr::Number(0.0), Expr::Number(0.0), Expr::Number(0.0)]
}

impl Datum {
    /// An axis-aligned plane at `offset` along `axis` — the lane-stack case.
    ///
    /// `[datum_plane "femur_inner" y femur_inner]` becomes a plane whose
    /// origin is `(0, femur_inner, 0)` and whose normal is `(0, 1, 0)`.
    pub fn axis_plane(axis: PrincipalAxis, offset: Expr) -> Self {
        let mut origin = zeros();
        origin[axis.index()] = offset;
        let u = axis.unit();
        Self::Plane {
            origin,
            normal: [Expr::Number(u[0]), Expr::Number(u[1]), Expr::Number(u[2])],
        }
    }

    /// An axis-aligned datum axis through the origin.
    pub fn principal_axis(axis: PrincipalAxis) -> Self {
        let u = axis.unit();
        Self::Axis {
            origin: zeros(),
            direction: [Expr::Number(u[0]), Expr::Number(u[1]), Expr::Number(u[2])],
        }
    }

    /// Variant name, for diagnostics.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Plane { .. } => "plane",
            Self::Axis { .. } => "axis",
            Self::Point { .. } => "point",
        }
    }

    /// Evaluate every coordinate against a resolved parameter environment.
    pub fn resolve(
        &self,
        name: &str,
        env: &HashMap<String, f64>,
    ) -> Result<ResolvedDatum, ResolveError> {
        let eval3 = |v: &[Expr; 3]| -> Result<[f64; 3], ResolveError> {
            let mut out = [0.0; 3];
            for (i, e) in v.iter().enumerate() {
                out[i] = eval_expr(name, e, env)?;
            }
            Ok(out)
        };
        Ok(match self {
            Self::Plane { origin, normal } => ResolvedDatum::Plane {
                origin: eval3(origin)?,
                normal: eval3(normal)?,
            },
            Self::Axis { origin, direction } => ResolvedDatum::Axis {
                origin: eval3(origin)?,
                direction: eval3(direction)?,
            },
            Self::Point { position } => ResolvedDatum::Point {
                position: eval3(position)?,
            },
        })
    }
}

/// Evaluate one datum coordinate, reporting failures against the datum name.
fn eval_expr(name: &str, e: &Expr, env: &HashMap<String, f64>) -> Result<f64, ResolveError> {
    match e {
        Expr::Number(v) => Ok(*v),
        Expr::Formula(s) => {
            let ast = crate::expr_parser::parse(s).map_err(|err| ResolveError::ParameterParse {
                name: format!("datum '{name}'"),
                message: err.to_string(),
            })?;
            crate::expr_parser::eval(&ast, env).map_err(|err| ResolveError::ParameterEval {
                name: format!("datum '{name}'"),
                message: err.to_string(),
            })
        }
    }
}

/// A datum with every coordinate evaluated to a concrete number.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolvedDatum {
    /// Resolved plane.
    Plane {
        /// A point on the plane.
        origin: [f64; 3],
        /// Plane normal.
        normal: [f64; 3],
    },
    /// Resolved axis.
    Axis {
        /// A point on the axis.
        origin: [f64; 3],
        /// Axis direction.
        direction: [f64; 3],
    },
    /// Resolved point.
    Point {
        /// The point's position.
        position: [f64; 3],
    },
}

/// Resolve every datum in a document against a parameter environment.
pub fn resolve_datums(
    datums: &HashMap<String, Datum>,
    env: &HashMap<String, f64>,
) -> Result<HashMap<String, ResolvedDatum>, ResolveError> {
    let mut out = HashMap::with_capacity(datums.len());
    for (name, d) in datums {
        out.insert(name.clone(), d.resolve(name, env)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axis_plane_keeps_offset_symbolic() {
        let d = Datum::axis_plane(PrincipalAxis::Y, Expr::formula("femur_inner"));
        let Datum::Plane { origin, normal } = &d else {
            panic!("expected plane");
        };
        assert_eq!(origin[1], Expr::formula("femur_inner"));
        assert_eq!(normal[1], Expr::Number(1.0));
    }

    #[test]
    fn resolves_through_the_parameter_env() {
        let d = Datum::axis_plane(PrincipalAxis::Y, Expr::formula("femur_inner + 3"));
        let env: HashMap<String, f64> = [("femur_inner".to_string(), 131.0)].into_iter().collect();
        let r = d.resolve("outer", &env).unwrap();
        let ResolvedDatum::Plane { origin, .. } = r else {
            panic!("expected plane");
        };
        assert_eq!(origin, [0.0, 134.0, 0.0]);
    }

    #[test]
    fn unknown_variable_is_an_error_not_a_zero() {
        let d = Datum::axis_plane(PrincipalAxis::Z, Expr::formula("nope"));
        assert!(d.resolve("d", &HashMap::new()).is_err());
    }

    #[test]
    fn round_trips_through_json() {
        let d = Datum::axis_plane(PrincipalAxis::X, Expr::formula("a + 1"));
        let s = serde_json::to_string(&d).unwrap();
        assert_eq!(d, serde_json::from_str::<Datum>(&s).unwrap());
    }
}
