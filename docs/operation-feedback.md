# Operation feedback

Progress is owned by an operation, not inferred from a Promise disappearing or a dialog unmounting.

| Lifecycle | Surface | Closing behavior |
| --- | --- | --- |
| Background work | Bottom-right activity and explicit terminal result | Navigation does not cancel work. Completion includes a next-step suggestion; results are dismissible. |
| Cancellable foreground work | One inline progress notice | Explain the actual cancellation action. Link discovery discards the preview and cleans its plan when it arrives; update download uses the native cancel command. |
| Non-cancellable foreground work | One inline progress notice | Disable dismissal while executing; ask the user not to quit the app. Do not claim that closing cancels a native write. |
| Ordinary data loading | Local loading/empty/error state | No global operation notification. |

Current routing:

- Manual Rescan is background work. Only a run observed in progress can produce a notification. Incomplete, cancelled and superseded runs are not success. A completed run waits for its matching published report.
- Repository restore/remove, confirmed batch local copies, global target quick actions and manual app-update checks use background notifications. Batch copying continues after the repository card unmounts; picker cancellation starts no work. Partial results retain created copies and say so.
- Git fetch, install, update, promotion and undo remain foreground transactions. Do not move these into background tasks just because the Tauri command uses a worker thread.
- Link apply, relocation/removal, global/project enable sheets, broken-member disable, Agent configuration writes, Home recovery and onboarding retain their existing foreground lifecycle.
- App-update download is cancellable; cancellation and installation themselves block dismissal.

Never duplicate foreground progress in the global panel. Keep previews, safety confirmations, actionable error details and Undo results; these are not redundant loading messages. Use no fabricated percentage or completion inferred from a cleared busy flag.
