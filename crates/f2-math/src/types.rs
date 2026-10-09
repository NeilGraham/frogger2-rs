//! Maths-related data types from `Common/types.h`, `Common/fixed.h`, `PC/libgte.h`, and
//! `Common/types.c`, plus the global constant vectors from `Common/frogger.c`.

use serde::{Deserialize, Serialize};

use crate::fixed::Fixed;

/// Port of `X` (Common/types.h:49).
pub const X: usize = 0;
/// Port of `Y` (Common/types.h:50).
pub const Y: usize = 1;
/// Port of `Z` (Common/types.h:51).
pub const Z: usize = 2;

/// Port of `FVECTOR` (Common/fixed.h:12): fixed-point 3D vector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FVector {
    pub vx: Fixed,
    pub vy: Fixed,
    pub vz: Fixed,
}

/// Port of `SVECTOR` (PC/libgte.h:15): short 3D vector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SVector {
    pub vx: i16,
    pub vy: i16,
    pub vz: i16,
    pub pad: i16,
}

/// Port of `VECTOR` (PC/libgte.h:9): long 3D vector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Vector {
    pub vx: i32,
    pub vy: i32,
    pub vz: i32,
    pub pad: i32,
}

/// Port of `IQUATERNION` (Common/types.h:117): 4096-scaled quaternion (or axis + angle).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IQuaternion {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub w: i32,
}

/// Port of `MATRIXI` (Common/types.h:122).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MatrixI {
    pub matrix: [[i32; 4]; 4],
}

/// Port of `POINT2D` (Common/types.h:172).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point2D {
    pub x: Fixed,
    pub z: Fixed,
}

/// Port of `BOX` (Common/types.h:177). Renamed to avoid clashing with `std::boxed::Box`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Box3D {
    pub pos: SVector,
    pub size: Vector,
}

/// Porting-guide aliases.
pub type FVec3 = FVector;
/// Porting-guide alias for [`SVector`].
pub type SVec3 = SVector;
/// Porting-guide alias for [`IQuaternion`].
pub type IQuat = IQuaternion;

impl FVector {
    /// Convenience constructor.
    #[inline]
    pub const fn new(vx: Fixed, vy: Fixed, vz: Fixed) -> Self {
        Self { vx, vy, vz }
    }
}

impl SVector {
    /// Convenience constructor (`pad` = 0).
    #[inline]
    pub const fn new(vx: i16, vy: i16, vz: i16) -> Self {
        Self { vx, vy, vz, pad: 0 }
    }
}

impl Vector {
    /// Convenience constructor (`pad` = 0).
    #[inline]
    pub const fn new(vx: i32, vy: i32, vz: i32) -> Self {
        Self { vx, vy, vz, pad: 0 }
    }
}

impl IQuaternion {
    /// Convenience constructor.
    #[inline]
    pub const fn new(x: i32, y: i32, z: i32, w: i32) -> Self {
        Self { x, y, z, w }
    }
}

/// Port of `MakeVECTOR` (Common/types.c:3). `pad` (uninitialised in C) is 0.
#[inline]
pub const fn make_vector(x: i32, y: i32, z: i32) -> Vector {
    Vector::new(x, y, z)
}

/// Port of `MakeSVECTOR` (Common/types.c:12). `pad` (uninitialised in C) is 0.
#[inline]
pub const fn make_svector(x: i16, y: i16, z: i16) -> SVector {
    SVector::new(x, y, z)
}

/// Port of `MakeFVECTOR` (Common/types.c:21).
#[inline]
pub const fn make_fvector(x: Fixed, y: Fixed, z: Fixed) -> FVector {
    FVector::new(x, y, z)
}

/// Initial value of global `zero` (Common/frogger.c:53).
pub const ZERO: Vector = Vector::new(0, 0, 0);
/// Initial value of global `vertQ` (Common/frogger.c:57). Note: the original global is
/// mutated by `CalculateQuatForPlane2`, so game state must own a copy.
pub const VERT_Q: IQuaternion = IQuaternion::new(0, 4096, 0, 0);
/// Port of global `upVec` (Common/frogger.c:64).
pub const UP_VEC: FVector = FVector::new(0, 4096, 0);
/// Port of global `inVec` (Common/frogger.c:65).
pub const IN_VEC: FVector = FVector::new(0, 0, 4096);
/// Port of global `outVec` (Common/frogger.c:66).
pub const OUT_VEC: FVector = FVector::new(0, 0, -4096);
/// Port of global `rightVec` (Common/frogger.c:67).
pub const RIGHT_VEC: FVector = FVector::new(4096, 0, 0);
/// Port of global `oneVec` (Common/frogger.c:68).
pub const ONE_VEC: FVector = FVector::new(4096, 4096, 4096);

/// Port of `fPI` (Common/maths.h:13).
pub const F_PI: Fixed = 12868;
/// Port of `fPI_OVER_2` (Common/maths.h:14).
pub const F_PI_OVER_2: Fixed = 6434;
/// Port of `fPI_OVER_4` (Common/maths.h:15).
pub const F_PI_OVER_4: Fixed = 3217;
/// Port of `fPI2` (Common/maths.h:16).
pub const F_PI2: Fixed = 25736;
/// Port of `SCALE` (Common/newpsx.h:6).
pub const SCALE: i32 = 10;
/// Port of `MAX_STACK_SIZE` (Common/maths.h:61).
pub const MAX_STACK_SIZE: usize = 16;
