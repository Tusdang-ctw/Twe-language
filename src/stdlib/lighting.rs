//! web3d (2026-10-08): the `light.*` / `sun.*` lighting builtins,
//! split out of the stdlib god-module. Point/sun state lives in the
//! parent's thread-locals, reached here via `use super::*`.

use super::*;

pub(super) fn light_add_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "light.add")?;
    let at = xyz_of(&args[0], "light.add.at")?;
    let color = rgba_of(&args[1], "light.add.color")?;
    let radius = number(&args[2], "light.add.radius")? as f32;
    if radius <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.add: radius must be > 0".to_string(),
            help: Some("a 0-radius light is disabled by definition".to_string()),
        });
    }
    let light =
        crate::render3d_types::PointLightU::point(at, [color[0], color[1], color[2]], radius);
    let handle = POINT_LIGHTS.with(|s| {
        let mut lights = s.borrow_mut();
        if let Some(i) = lights.iter().position(|l| l.color_radius[3] <= 0.0) {
            lights[i] = light;
            return Some(i + 1);
        }
        if lights.len() < crate::render3d_types::MAX_LIGHTS {
            lights.push(light);
            return Some(lights.len());
        }
        None
    });
    match handle {
        Some(h) => Ok(Value::from_int(h as i64)),
        None => Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "light.add: all {} light slots are in use",
                crate::render3d_types::MAX_LIGHTS
            ),
            help: Some("call light.remove(h) to free a slot".to_string()),
        }),
    }
}

pub(super) fn light_remove_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.remove")?;
    let handle = handle_int(&args[0], "light.remove")?;
    with_light(handle, |slot| *slot = bytemuck::Zeroable::zeroed());
    Ok(Value::NIL)
}

pub(super) fn light_ambient_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.ambient")?;
    let color = rgba_of(&args[0], "light.ambient.color")?;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        st.ambient = [color[0], color[1], color[2], 0.0];
    });
    Ok(Value::NIL)
}

pub(super) fn light_set_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 4, "light.set")?;
    let handle = handle_int(&args[0], "light.set")?;
    let at = xyz_of(&args[1], "light.set.at")?;
    let color = rgba_of(&args[2], "light.set.color")?;
    let radius = number(&args[3], "light.set.radius")? as f32;
    with_light(handle, |slot| {
        // pos.w is the light's shadow flag (`light.shadow`) and the
        // cone stays: kept.
        slot.pos = [at[0], at[1], at[2], slot.pos[3]];
        slot.color_radius = [color[0], color[1], color[2], radius];
    });
    Ok(Value::NIL)
}

// web3d-M7: `light.fog(density, falloff, color)` — exponential height
// fog: `density` per world unit at y = 0, thinning by e^(-falloff * y)
// with height (0 = uniform fog). Density 0 turns it off.
pub(super) fn light_fog_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "light.fog")?;
    let density = number(&args[0], "light.fog.density")? as f32;
    let falloff = number(&args[1], "light.fog.falloff")? as f32;
    if density < 0.0 || falloff < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.fog: density and falloff can't be negative".to_string(),
            help: Some("e.g. `light.fog(0.05, 0.3, (0.7, 0.75, 0.8))`; `light.fog(0, 0, color.white)` turns it off".to_string()),
        });
    }
    let (r, g, b, _) = rgba(&args[2], "light.fog.color")?;
    let color = [r as f32, g as f32, b as f32];
    FOG.with(|s| *s.borrow_mut() = (density > 0.0).then_some((density, falloff, color)));
    Ok(Value::NIL)
}

// web3d-M7: `light.volumetric(enabled)` — light the fog per point in
// space: shafts where the sun passes shadow casters, halos around point
// and spot lights. It changes how `light.fog`'s fog is lit, so it needs
// fog to show.
pub(super) fn light_volumetric_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.volumetric")?;
    if !args[0].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: format!(
                "light.volumetric expects true or false, got {}",
                args[0].type_name()
            ),
            help: Some("e.g. `light.volumetric(true)` (with fog from `light.fog`)".to_string()),
        });
    }
    let on = args[0].as_bool();
    FOG_VOLUMETRIC.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}

// web3d-M7 session 16: `light.environment(path, intensity)` — light the
// scene from an equirectangular HDR image (Radiance `.hdr`): diffuse
// light from every direction and reflections in glossy surfaces
// (image-based lighting). It replaces `light.ambient`'s flat colour;
// the sun and point lights still add. Intensity 0 turns it off.
pub(super) fn light_environment_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "light.environment")?;
    let help = "e.g. `light.environment(\"assets/sky.hdr\", 1.0)`; intensity 0 turns it off";
    if !args[0].is_str() || !args[0].as_string().ends_with(".hdr") {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.environment expects the path of a Radiance .hdr image".to_string(),
            help: Some(help.to_string()),
        });
    }
    let intensity = number(&args[1], "light.environment.intensity")? as f32;
    if intensity < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.environment: intensity can't be negative".to_string(),
            help: Some(help.to_string()),
        });
    }
    let path = args[0].as_string();
    ENVIRONMENT.with(|s| *s.borrow_mut() = (intensity > 0.0).then_some((path, intensity)));
    Ok(Value::NIL)
}

pub(super) fn light_shadow_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "light.shadow")?;
    let handle = handle_int(&args[0], "light.shadow")?;
    if !args[1].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.shadow expects a bool".to_string(),
            help: Some("`light.shadow(torch, true)` turns the light's shadows on".to_string()),
        });
    }
    let on = args[1].as_bool();
    with_light(handle, |slot| slot.pos[3] = if on { 1.0 } else { 0.0 });
    Ok(Value::NIL)
}

// web3d-M7: `light.cone(handle, direction, angle)` — makes the light a
// spot light shining along `direction`, lighting a cone of `angle`
// degrees either side of it (the edge fades over its outer fifth).
// An angle of 180 or more makes it a point light again.
pub(super) fn light_cone_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 3, "light.cone")?;
    let handle = handle_int(&args[0], "light.cone")?;
    let dir = xyz_of(&args[1], "light.cone.direction")?;
    let angle = number(&args[2], "light.cone.angle")? as f32;
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len < 1e-6 || angle <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "light.cone needs a direction and an angle above 0".to_string(),
            help: Some("e.g. `light.cone(lamp, vec3(0, -1, 0), 30)` shines straight down in a 60-degree cone".to_string()),
        });
    }
    let (cone, params) = if angle >= 180.0 {
        ([0.0, -1.0, 0.0, -2.0], [-2.0, 0.0, 0.0, 0.0])
    } else {
        let outer = angle.to_radians();
        let inner = outer * 0.8;
        (
            [dir[0] / len, dir[1] / len, dir[2] / len, outer.cos()],
            [inner.cos(), 0.0, 0.0, 0.0],
        )
    };
    with_light(handle, |slot| {
        slot.cone = cone;
        slot.params = params;
    });
    Ok(Value::NIL)
}

pub(super) fn light_set_radius_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "light.set_radius")?;
    let handle = handle_int(&args[0], "light.set_radius")?;
    let radius = number(&args[1], "light.set_radius.radius")? as f32;
    with_light(handle, |slot| slot.color_radius[3] = radius);
    Ok(Value::NIL)
}

// Lanternfall E6: `light.background(color)` — what a 3D frame is drawn
// over where nothing covers it (and what plain height fog fades
// toward). An sRGB colour, as every script colour is.
pub(super) fn light_background_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "light.background")?;
    let (r, g, b, _) = rgba(&args[0], "light.background.color")?;
    let linear = [srgb_to_linear(r) as f32, srgb_to_linear(g) as f32, srgb_to_linear(b) as f32];
    BACKGROUND.with(|s| *s.borrow_mut() = linear);
    Ok(Value::NIL)
}

pub(super) fn light_clear_impl(_env: &mut Env, _args: &[Value]) -> Result<Value, RuntimeError> {
    POINT_LIGHTS.with(|s| s.borrow_mut().clear());
    Ok(Value::NIL)
}

pub(super) fn sun_direction_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.direction")?;
    let v = xyz_of(&args[0], "sun.direction.v")?;
    LIGHTS_STATE.with(|s| {
        let mut st = s.borrow_mut();
        // Normalize so the shader doesn't have to. Zero-length
        // vector falls back to a sensible up-vector.
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len > 1e-6 {
            st.sun_dir[0] = v[0] / len;
            st.sun_dir[1] = v[1] / len;
            st.sun_dir[2] = v[2] / len;
        } else {
            st.sun_dir[0] = 0.0;
            st.sun_dir[1] = 1.0;
            st.sun_dir[2] = 0.0;
        }
    });
    Ok(Value::NIL)
}

pub(super) fn sun_intensity_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.intensity")?;
    let i = number(&args[0], "sun.intensity.i")? as f32;
    LIGHTS_STATE.with(|s| {
        s.borrow_mut().sun_dir[3] = i.max(0.0);
    });
    Ok(Value::NIL)
}

pub(super) fn sun_shadow_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.shadow")?;
    let on = if args[0].is_bool() {
        args[0].as_bool()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "sun.shadow expects a bool (sun.shadow(true) or sun.shadow(false))"
                .to_string(),
            help: None,
        });
    };
    SHADOW_ENABLED.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}

pub(super) fn sun_shadow_extent_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "sun.shadow_extent")?;
    let r = number(&args[0], "sun.shadow_extent.radius")? as f32;
    if r > 0.0 {
        SHADOW_EXTENT.with(|s| *s.borrow_mut() = r);
    }
    Ok(Value::NIL)
}

// web3d-M7: the tonemap curve by name ("aces", "agx", "neutral",
// "none"); `true` / `false` keep their Phase 26 meaning (ACES / none).
pub(super) fn postfx_tonemap_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    use crate::kernel::render::Tonemapper;
    arity(args, 1, "postfx.tonemap")?;
    let bad = |got: String| {
        RuntimeError {
        line: 0,
        col: 0,
        message: format!("postfx.tonemap: unknown curve {got}"),
        help: Some(
            "use \"aces\" (default), \"agx\", \"neutral\" or \"none\" (true / false mean \"aces\" / \"none\")"
                .to_string(),
        ),
    }
    };
    let curve = if args[0].is_bool() {
        if args[0].as_bool() {
            Tonemapper::Aces
        } else {
            Tonemapper::None
        }
    } else if args[0].is_str() {
        match args[0].as_string().as_str() {
            "aces" => Tonemapper::Aces,
            "agx" => Tonemapper::AgX,
            "neutral" => Tonemapper::Neutral,
            "none" => Tonemapper::None,
            other => return Err(bad(format!("\"{other}\""))),
        }
    } else {
        return Err(bad("(expected a string or a bool)".to_string()));
    };
    TONEMAPPER.with(|s| *s.borrow_mut() = curve);
    Ok(Value::NIL)
}

pub(super) fn postfx_exposure_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.exposure")?;
    let stops = (number(&args[0], "postfx.exposure.stops")? as f32).clamp(-16.0, 16.0);
    EXPOSURE_STOPS.with(|s| *s.borrow_mut() = stops);
    Ok(Value::NIL)
}

pub(super) fn postfx_auto_exposure_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.auto_exposure")?;
    if !args[0].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.auto_exposure expects a bool".to_string(),
            help: Some(
                "`postfx.auto_exposure(true)` adapts exposure to the scene's brightness"
                    .to_string(),
            ),
        });
    }
    AUTO_EXPOSURE.with(|s| *s.borrow_mut() = args[0].as_bool());
    Ok(Value::NIL)
}

pub(super) fn postfx_ao_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.ao")?;
    let v = (number(&args[0], "postfx.ao.intensity")? as f32).clamp(0.0, 4.0);
    AO_INTENSITY.with(|s| *s.borrow_mut() = v);
    Ok(Value::NIL)
}

// web3d-M7: `postfx.dof(focus, f_stop)` — a thin lens focused at
// `focus` world units with aperture f/`f_stop` on a full-frame sensor
// (focal length from the camera's field of view). `postfx.dof(0, 0)`
// turns it off.
pub(super) fn postfx_dof_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "postfx.dof")?;
    let focus = number(&args[0], "postfx.dof.focus")? as f32;
    let f_stop = number(&args[1], "postfx.dof.f_stop")? as f32;
    if focus < 0.0 || f_stop < 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.dof: focus and f_stop can't be negative".to_string(),
            help: Some("e.g. `postfx.dof(8, 1.4)` focuses 8 m away at f/1.4; `postfx.dof(0, 0)` turns it off".to_string()),
        });
    }
    DOF.with(|s| *s.borrow_mut() = (focus, f_stop));
    Ok(Value::NIL)
}

// web3d-M7: camera motion blur; the fraction of a frame the shutter is
// open (0.5 = a film camera's 180-degree shutter), 0 = off.
pub(super) fn postfx_motion_blur_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.motion_blur")?;
    let shutter = (number(&args[0], "postfx.motion_blur.shutter")? as f32).clamp(0.0, 2.0);
    MOTION_BLUR.with(|s| *s.borrow_mut() = shutter);
    Ok(Value::NIL)
}

// web3d-M7: screen-space reflections: how much of a surface's
// environment reflection is replaced by what the screen shows along the
// reflected ray (0 = off, 1 = all of it where a ray hits).
pub(super) fn postfx_ssr_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.ssr")?;
    let strength = (number(&args[0], "postfx.ssr.strength")? as f32).clamp(0.0, 1.0);
    SSR.with(|s| *s.borrow_mut() = strength);
    Ok(Value::NIL)
}

// web3d-M7: colour grading through a `.cube` 3D LUT; strength 0..1
// (0 turns it off).
pub(super) fn postfx_lut_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 2, "postfx.lut")?;
    if !args[0].is_str() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.lut expects a path to a .cube file".to_string(),
            help: Some("e.g. `postfx.lut(\"looks/warm.cube\", 1.0)`".to_string()),
        });
    }
    let path = args[0].as_string();
    let strength = (number(&args[1], "postfx.lut.strength")? as f32).clamp(0.0, 1.0);
    COLOR_LUT.with(|s| *s.borrow_mut() = (strength > 0.0).then_some((path, strength)));
    Ok(Value::NIL)
}

pub(super) fn postfx_ao_radius_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.ao_radius")?;
    let r = number(&args[0], "postfx.ao_radius.radius")? as f32;
    if r <= 0.0 {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.ao_radius: radius must be > 0".to_string(),
            help: Some(
                "the radius is in world units (metres); 0.5 suits human-scale scenes".to_string(),
            ),
        });
    }
    AO_RADIUS.with(|s| *s.borrow_mut() = r);
    Ok(Value::NIL)
}

pub(super) fn postfx_taa_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.taa")?;
    if !args[0].is_bool() {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.taa expects a bool".to_string(),
            help: Some("`postfx.taa(true)` turns temporal anti-aliasing on".to_string()),
        });
    }
    TAA_ENABLED.with(|s| *s.borrow_mut() = args[0].as_bool());
    Ok(Value::NIL)
}

pub(super) fn postfx_vignette_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.vignette")?;
    let s = (number(&args[0], "postfx.vignette.strength")? as f32).clamp(0.0, 1.0);
    VIGNETTE_STRENGTH.with(|st| *st.borrow_mut() = s);
    Ok(Value::NIL)
}

// Phase 28 session 4: vignette tint color. Accepts an (r, g, b)
// or (r, g, b, a) tuple — alpha is ignored. Components clamp to
// [0, 1] so any color literal or color.* constant is valid input.
pub(super) fn postfx_vignette_color_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.vignette_color")?;
    let (r, g, b, _a) = rgba(&args[0], "postfx.vignette_color.color")?;
    VIGNETTE_COLOR.with(|c| {
        *c.borrow_mut() = [
            (r as f32).clamp(0.0, 1.0),
            (g as f32).clamp(0.0, 1.0),
            (b as f32).clamp(0.0, 1.0),
        ]
    });
    Ok(Value::NIL)
}

// Phase 28 session 3: bloom intensity. 0 disables (default). 1.0
// is a strong-but-not-overpowering value for typical scenes.
pub(super) fn postfx_bloom_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.bloom")?;
    let s = (number(&args[0], "postfx.bloom.intensity")? as f32).max(0.0);
    BLOOM_INTENSITY.with(|st| *st.borrow_mut() = s);
    Ok(Value::NIL)
}

// Phase 28 session 3: bloom threshold. HDR luminance below this
// value contributes nothing; above it, the excess blooms.
pub(super) fn postfx_bloom_threshold_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.bloom_threshold")?;
    let t = (number(&args[0], "postfx.bloom_threshold.threshold")? as f32).max(0.0);
    BLOOM_THRESHOLD.with(|st| *st.borrow_mut() = t);
    Ok(Value::NIL)
}

pub(super) fn postfx_frustum_cull_impl(_env: &mut Env, args: &[Value]) -> Result<Value, RuntimeError> {
    arity(args, 1, "postfx.frustum_cull")?;
    let on = if args[0].is_bool() {
        args[0].as_bool()
    } else {
        return Err(RuntimeError {
            line: 0,
            col: 0,
            message: "postfx.frustum_cull expects a bool".to_string(),
            help: None,
        });
    };
    FRUSTUM_CULL_ENABLED.with(|s| *s.borrow_mut() = on);
    Ok(Value::NIL)
}
