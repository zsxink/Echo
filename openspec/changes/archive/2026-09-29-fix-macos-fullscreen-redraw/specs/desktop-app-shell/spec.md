## ADDED Requirements

### Requirement: Background close exits macOS native fullscreen before hiding the main window
When the macOS close behavior is set to background, the desktop shell SHALL exit native fullscreen before hiding the main window. It SHALL keep the app and playback session running, and later show the main window in its normal windowed state.

#### Scenario: Close the main window from native fullscreen
- **WHEN** the user clicks the red close button while the main window is in native fullscreen and macOS background behavior is active
- **THEN** Echo exits native fullscreen, hides the main window after the exit completes, remains available in the menu bar, and keeps playback uninterrupted

#### Scenario: Reopen after closing from native fullscreen
- **WHEN** the user shows the main window again from the Dock or menu bar after a fullscreen background close
- **THEN** the main window appears at its restored windowed size and position with the current view and playback session intact

#### Scenario: Background close while windowed
- **WHEN** the user clicks the red close button while the main window is already windowed and background behavior is active
- **THEN** Echo hides the main window immediately and continues running with playback uninterrupted
