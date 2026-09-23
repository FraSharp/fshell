# fshell downstream crossterm fixes

This source tree is crossterm 0.29.0, retained as a small downstream fork until
the Unix terminal EOF fixes are available in an upstream release.

The Unix event sources now report terminal closure when reading returns EOF,
propagate terminal read errors, and the `EventStream` worker wakes its consumer
on source errors. This prevents a closed PTY from leaving a polling thread in an
unbounded retry loop. Keep the upstream package metadata, license, and source
files intact. Once an upstream release contains equivalent behavior, remove
this override and update the workspace dependency.
