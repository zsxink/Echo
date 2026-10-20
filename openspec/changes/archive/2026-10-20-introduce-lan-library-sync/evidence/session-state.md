# Sync session state transitions

Implemented by the pure `transition_sync_session` function in Core. Invalid
state/event combinations return `InvalidSyncTransition` and do not mutate state.

| Current state | Event | Next state |
|---|---|---|
| Disabled | Enable | Ready |
| Ready | PairingRequired | AwaitingPairing |
| Ready / AwaitingPairing | BeginPairing | Pairing |
| Pairing | PairingAccepted | Enrolling |
| Pairing / Enrolling | Cancel | Ready |
| Ready | BeginEnrollment | Enrolling |
| Enrolling | EnrollmentComplete | Synchronizing |
| Synchronizing | Pause | Paused |
| Synchronizing / Paused | HostUnavailable | Offline |
| Offline | Reconnected | Synchronizing |
| Offline | Pause | Paused |
| Synchronizing / Paused / Offline | Cancel | Ready |
| Active transfer phases | Failure | Failed |
| Failed | Retry | Synchronizing |
| Any enabled state | Disable | Disabled |

Events operate on scheduling/session state only. A database mutation committed
before a cancellation remains pending for later retry. Core transition tests
cover offline recovery, cancellation, pause, disable, and illegal event order.
