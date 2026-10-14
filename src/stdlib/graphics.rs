//! Low-end track (G2): the `graphics.*` builtins — the GPU budget a
//! game asks for. The tier lives in a thread-local here; the 3D host
//! hands it to the renderer every frame (`host3d::render_frame`).
//! Tiers themselves are defined by the kernel (`kernel/quality.rs`).

use super::*;
use crate::kernel::quality::{Quality, Tier};

thread_local! {
    /// The game's choice: a tier, and an optional render-scale override.
    /// `None` = the host's default.
    static TIER: RefCell<Option<Tier>> = const { RefCell::new(None) };
    static RENDER_SCALE: RefCell<Option<f32>> = const { RefCell::new(None) };
}

/// The budget for the next frame: the script's tier (else `TWE_QUALITY`
/// natively, else `Ultra`), with its render-scale override applied.
pub fn graphics_quality() -> Quality {
    let tier = TIER
        .with(|t| *t.borrow())
        .or_else(env_tier)
        .unwrap_or(Tier::Ultra);
    let mut q = Quality::tier(tier);
    if let Some(s) = RENDER_SCALE.with(|s| *s.borrow()) {
        q.render_scale = s;
    }
    q
}

/// `TWE_QUALITY=low|medium|high|ultra` picks the tier for games that
/// don't (benchmarks, trying a game on a weaker setting).
fn env_tier() -> Option<Tier> {
    if cfg!(target_arch = "wasm32") {
        return None;
    }
    std::env::var("TWE_QUALITY").ok().and_then(|v| Tier::parse(&v.to_ascii_lowercase()))
}

pub(super) fn graphics_quality_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "graphics.quality")?;
    let name = string_arg(&args[0], "graphics.quality", "tier name")?;
    let tier = Tier::parse(&name).ok_or_else(|| RuntimeError {
        line: 0,
        col: 0,
        message: format!("graphics.quality: unknown tier \"{name}\""),
        help: Some("one of \"low\", \"medium\", \"high\", \"ultra\"".to_string()),
    })?;
    TIER.with(|t| *t.borrow_mut() = Some(tier));
    Ok(Value::NIL)
}

pub(super) fn graphics_render_scale_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "graphics.render_scale")?;
    let scale = number(&args[0], "graphics.render_scale")?;
    if !(0.25..=1.0).contains(&scale) {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!("graphics.render_scale: {scale} is outside 0.25 – 1.0"),
            help: Some("1.0 renders at the window's size; 0.5 at half of it per axis".to_string()),
        });
    }
    RENDER_SCALE.with(|s| *s.borrow_mut() = Some(scale as f32));
    Ok(Value::NIL)
}

pub(super) fn graphics_tier_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 0, "graphics.tier")?;
    Ok(Value::from_string(graphics_quality().tier.name().to_string()))
}
