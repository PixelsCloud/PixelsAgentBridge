"""Explicit Release presets shared by local and publication build entry points."""

PRESETS = {
    "fast": {"OPT_LEVEL": "2", "LTO": "false", "CODEGEN_UNITS": "16", "INCREMENTAL": "true"},
    "optimized": {"OPT_LEVEL": "3", "LTO": "thin", "CODEGEN_UNITS": "1", "INCREMENTAL": "false"},
}


def apply_release_preset(env, preset=None, *, publication=False):
    preset = preset or ("optimized" if publication else env.get("PAB_BUILD_PRESET", "fast"))
    if publication and preset != "optimized":
        raise ValueError("Public releases require the optimized Release preset")
    settings = PRESETS[preset]
    env["PAB_BUILD_PRESET"] = preset
    env["CARGO_INCREMENTAL"] = "1" if settings["INCREMENTAL"] == "true" else "0"
    for name, value in settings.items():
        env["CARGO_PROFILE_RELEASE_" + name] = value
    print(f"Release preset: {preset} ({settings})", flush=True)
    return env
