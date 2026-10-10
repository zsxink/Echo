// Validate captured facts instead of trusting a runtime_confirmed/harness flag.
export function runtimeEvidenceProblems(evidence) {
  const failures = [];
  if (evidence.gate_passed !== false) failures.push("runtime proof must not claim the complete native Gate");
  const expectedChanges = { equalizer: 6, preamp: -6, spatial_width: -20 * Math.log10(1.25) };
  const validWindow = (window, total) => Array.isArray(window) && window.length === 2
    && window.every(Number.isInteger) && window[0] >= 0 && window[1] > window[0] && window[1] <= total;
  for (const [parameter, expected] of Object.entries(expectedChanges)) {
    const check = evidence.checks?.find((item) => item.parameter === parameter);
    if (!check || check.runtime_confirmed !== true || !Number.isInteger(check.command_result)
        || check.command_result < 0 || check.eof_before_command !== false
        || !Number.isFinite(check.error_db) || Math.abs(check.error_db) > 0.25
        || !Number.isFinite(check.measured_change_db) || !Number.isFinite(check.expected_change_db)
        || Math.abs(check.expected_change_db - expected) > 1e-9
        || Math.abs(check.measured_change_db - check.expected_change_db - check.error_db) > 1e-9
        || !Number.isInteger(check.output_frames) || !Number.isInteger(check.command_frame_upper_bound)
        || !validWindow(check.baseline_window_frames, check.output_frames)
        || !validWindow(check.changed_window_frames, check.output_frames)
        || check.baseline_window_frames[1] > check.command_frame_upper_bound
        || check.changed_window_frames[0] <= check.command_frame_upper_bound) {
      failures.push(`AFX-9.5 lacks consistent pre-EOF PCM measurements for ${parameter}`);
    }
  }
  const control = evidence.checks?.find((check) => check.name === "static_gain_reference");
  if (!control || control.harness_valid !== true || !Number.isFinite(control.measured_db)
      || !Number.isFinite(control.expected_db) || Math.abs(control.expected_db - 6) > 1e-9
      || !Number.isFinite(control.error_db) || Math.abs(control.error_db) > 0.25
      || Math.abs(control.measured_db - control.expected_db - control.error_db) > 1e-9) {
    failures.push("runtime PCM evidence has no consistent static-gain control");
  }
  return failures;
}

export function taskIdProblems(ids, manifestIds) {
  const failures = [];
  const bare = [...new Set(ids)].filter((id) => !/^AFX-/.test(id) && manifestIds.has(id));
  if (bare.length) failures.push(`bare audio-effects task ids collide with unrelated manifest ids: ${bare.join(", ")}`);
  const duplicates = ids.filter((id, index) => ids.indexOf(id) !== index);
  if (duplicates.length) failures.push(`duplicate audio-effects task ids: ${[...new Set(duplicates)].join(", ")}`);
  return failures;
}
