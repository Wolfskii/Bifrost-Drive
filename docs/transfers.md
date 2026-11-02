# Transfers

The provider-neutral bounded transfer queue is implemented with durable-schema support. It models queue state, bounds concurrency, supports progress, pause/resume/cancel/retry, uses capped exponential backoff, persists snapshots, restores interrupted jobs as pending, and streams provider I/O into atomic cache commits. Hydration, uploads, and recent activity are exposed by the desktop host. Byte-range resume checkpoints remain future work.

Activity and history must omit credentials, access tokens, and file contents.

## Batch transfers

`bifrost_transfer::batch` copies or moves folder trees between a local folder and any provider, between two providers, or between two local folders (a local move is a rename when it stays on one volume; local copies go through a partial file, keep the modification time, and fail rather than lose data if the source changed while being read) for the Explorer transfer window. A batch is planned first so totals are exact, then runs at most `max_concurrent` files at once (default 4, capped at 16). Network errors are retried with capped backoff; a file that still fails is reported with a plain reason and the rest continue. An existing destination is never overwritten silently: the batch asks (skip, replace, keep both, optionally for every clash) while other files keep going. Busy existing destination folders are listed once instead of checked per file. Pause holds new files back, cancel stops in-flight streams and removes partial files. A move removes each source only after its destination was written and size-checked, and removes source folders only when the whole batch was clean and the folder is empty (some providers delete recursively, so emptiness is checked first). Transfers between two servers stage through a temporary file; moves inside one server use the provider's rename when it supports it.

### Speed

Explorer works through one file at a time, and for small files the time goes into handling each file (create, close, security, one round trip to the server) rather than moving data. The batch engine runs many files at once and chooses how many from what the batch touches (`bifrost_transfer::concurrency`):

| Touches                                                    | Starts at              | May grow to |
| ---------------------------------------------------------- | ---------------------- | ----------- |
| A server drive (SFTP, S3, WebDAV, ...)                     | 8 files                | 16          |
| Two SSDs or NVMe drives                                    | 8 files                | 16          |
| A removable drive (USB stick, card)                        | 2 files                | 6           |
| A mapped network share, or a drive Windows cannot classify | 4 files                | 8           |
| A spinning disk on one side, other side local              | 1 file, in path order  | 1           |
| A spinning disk with a server on the other side            | 2 files, in path order | 4           |

The Windows adapter (`platforms/windows/storage`) classifies a local drive from its drive type and the storage stack's "incurs seek penalty" flag; when Windows cannot tell, the class is `Unknown` and the middle row applies. Above its starting value the engine measures throughput (bytes plus a fixed cost per file) every 500 ms and raises the number of files in flight while that pays off, doubling while each step gains at least 50%, and returns to the last good value as soon as a step does not help (`Tuner`). The hard bound is 16 files in flight, whatever the options say.

Small files that do not exist at the destination yet are written straight to their final name; replacements, unknown sizes and files over 32 MiB go through a `.bifrost-part` file and a rename, because creating and renaming a second file doubled the folder operations in measurements. Local files are copied with one blocking call per file so the operating system's own copy is used.

Deleting (`start_delete`) removes files in parallel with the same limits, then the emptied folders from the deepest level up; a folder that still holds anything is left alone. It is offered only for items on a Bifrost drive, never for a whole drive, and only after a confirmation.

#### Measurements

Run on the development machine (Windows 11, Samsung 990 PRO NVMe, Defender on); every number is one run, so treat differences under about 20% as noise. Re-run them with `task bench:local` and `task bench:sftp`.

Local, 5,000 files of 16 KB between folders on the same NVMe drive:

| Method                                                | Time               |
| ----------------------------------------------------- | ------------------ |
| Explorer's copy engine (`Shell.Application.CopyHere`) | 13.0 s             |
| `robocopy /MT:1`                                      | 2.7 s              |
| `robocopy /MT:8`                                      | 1.4 s              |
| Bifrost engine, adaptive from 8 to 16                 | about 2.0 to 2.3 s |

So the engine is several times faster than Explorer here but not faster than multi-threaded `robocopy`. Over SFTP to a server on the same machine (round trips as cheap as they get; a server across a network gains more), 3,000 files of 16 KB:

|          | One file at a time | App default (adaptive 8 to 16) |
| -------- | ------------------ | ------------------------------ |
| Upload   | 6.0 s              | 0.9 s                          |
| Download | 6.9 s              | 2.5 s                          |
| Delete   | 3.9 s              | 0.7 s                          |

Large files are unaffected by any of this: one stream already saturates the link or the disk, and parallelism only helps when files are small. The classification is unverified on spinning disks and USB sticks, because the development machine has neither.
