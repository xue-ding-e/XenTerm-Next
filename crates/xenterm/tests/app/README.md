# App unit tests

These tests are kept outside the modules they test but are compiled as child
modules through `#[path = "..."]` declarations. This preserves access to private
helpers without exposing them as production APIs.

The former desktop-toolkit fixtures (window geometry, window management, welcome
sidebar, command-history popovers, the process-monitor window, port-forwarding
dialogs, keyboard input) were deleted with the frontend they tested; what remains
covers the layers the service core still owns: terminal ingest pacing, output
highlighting, terminal rendering and the terminal benchmark.
