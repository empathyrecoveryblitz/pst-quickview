# Installing PST QuickView on macOS

PST QuickView v0.2.0-beta.2 is an unsigned and unnotarized public beta. Install it only from the project's [official GitHub release page](https://github.com/empathyrecoveryblitz/pst-quickview/releases/tag/v0.2.0-beta.2).

## System requirements

- A Mac with an Intel (`x86_64`) or Apple silicon (`arm64`) processor.
- macOS. The beta is distributed as a Universal application; the project does not yet claim a fully qualified minimum OS version across all workflows.
- Enough free disk space for the application and any local PST workspaces. PST import creates extracted message copies and a SQLite index, and the app presents a storage estimate before conversion.

The released application includes ReadPST / LibPST 0.6.76. Homebrew is not required for PST conversion.

## Install from the DMG

1. Open the [v0.2.0-beta.2 release page](https://github.com/empathyrecoveryblitz/pst-quickview/releases/tag/v0.2.0-beta.2).
2. Download the DMG from the release assets.
3. Open the downloaded DMG.
4. Drag **PST QuickView** into the **Applications** folder.
5. Eject the DMG after the copy finishes.

## First launch of the unsigned beta

Because this beta is not signed or notarized, macOS may block a normal double-click on first launch.

1. In Finder, open **Applications**.
2. Right-click or Control-click **PST QuickView**.
3. Choose **Open**.
4. Review the macOS warning and choose **Open** to approve this copy of the app.

This uses macOS's per-application approval flow. Do not disable Gatekeeper globally.

## If macOS says it cannot check the app for malicious software

The message “Apple cannot check it for malicious software” reflects the beta's unsigned and unnotarized status. It is not a claim that Apple or another party has reviewed the application.

1. Confirm that the DMG came from the official release page linked above.
2. Move any copy from an untrusted source to the Trash and download it again from GitHub.
3. Use Finder's right-click **Open** flow described above.
4. If macOS still blocks the app, open **System Settings > Privacy & Security** and use **Open Anyway** only for the PST QuickView launch you just attempted, if that option is shown.

Do not turn off Gatekeeper system-wide. If the per-app controls are unavailable or the download cannot be verified, stop and do not open that copy.

## Upgrade

1. Quit PST QuickView.
2. Download the newer DMG from the project's [Releases](https://github.com/empathyrecoveryblitz/pst-quickview/releases) page.
3. Open the DMG and drag **PST QuickView** into **Applications**.
4. Choose **Replace** when Finder asks about the existing application.
5. Launch the new copy. Because releases are currently unsigned, macOS may require per-app approval again.

Replacing the app does not remove original PST, EML, or MSG files, and it does not automatically remove existing local workspaces.

## Uninstall

1. Quit PST QuickView.
2. In Finder, move **PST QuickView** from **Applications** to the Trash.
3. Empty the Trash when appropriate.

Removing the application does not automatically remove local workspaces, logs, standalone attachment exports, or files exported to locations you selected.

### Remove local data when required

The preferred way to remove a PST workspace is to use **Delete Workspace** inside PST QuickView before uninstalling. That action is limited to a marked PST QuickView workspace and does not delete the original PST.

Depending on the selected workspace location, retained data may be found in:

- `~/Library/Application Support/PST QuickView/workspaces/`
- `.pst-quickview.noindex/` beside a source PST
- `.pst-quickview/` beside a source PST created by an older workspace format

The Application Support folder can also retain:

- local application logs under `~/Library/Application Support/PST QuickView/logs/`;
- standalone EML exports under `~/Library/Application Support/PST QuickView/eml-exports/`; and
- standalone MSG exports under `~/Library/Application Support/PST QuickView/msg-exports/`.

Files exported to another location remain wherever they were saved.

If manual cleanup is necessary, inspect each folder in Finder and move only data you recognize as PST QuickView workspace or export data to the Trash. Never delete the original PST, EML, or MSG source as part of workspace cleanup. Workspaces and exported attachments can contain sensitive message content; handle their removal accordingly.

For a complete description of retained local data, see [Privacy and local data](PRIVACY.md).
