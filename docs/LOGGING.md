# Logging Policy

PST QuickView logs operational metadata for troubleshooting. Logs must not contain message bodies,
sanitized or raw HTML, recipient/header content, search terms, attachment payloads, or raw source
contents.

## Application Log

Path:

```text
~/Library/Application Support/PST QuickView/logs/application.log
```

The application log records timestamps, operation names, stages, workspace identifiers when
available, and concise errors. It rotates at 1 MiB and retains two backups:

```text
application.log
application.log.1
application.log.2
```

## Workspace Import Log

Path:

```text
<workspace>/logs/import.log
```

The import log records import/reindex stages, workspace/PST paths, fingerprints, readpst
path/version, counts, cancellation, and errors. It also captures quiet-mode readpst stdout/stderr.
App-written import/index entries rotate at 10 MiB. Starting a new import also rotates the prior log.
Two backups are retained:

```text
import.log
import.log.1
import.log.2
```

`readpst` writes directly to the active log while it runs in quiet mode. An abnormal tool failure
can briefly exceed the 10 MiB checkpoint before PST QuickView regains control and rotates the log.

## Workspace Export Log

Path:

```text
<workspace>/logs/exports.log
```

The export log records operation type, local message/attachment IDs, sizes, timestamps, and errors.
It intentionally omits attachment filenames and exported output paths. It rotates at 2 MiB and
retains two backups.

## Retention And Deletion

- Application logs are retained until rotated or manually removed.
- Workspace logs are deleted when that workspace is safely deleted.
- Logs can be revealed from Help > Diagnostics > Reveal Logs.
- Removing log files does not modify source PST, EML, or MSG files.
