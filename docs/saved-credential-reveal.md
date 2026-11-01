# The saved-credential reveal preference

The profile format carries an **Allow viewing saved credentials** preference
(`allow_secret_reveal`), off by default. Profiles created before the preference
existed also default to off. Enabling it only grants local desktop-UI permission
to reveal a saved value; it never enables plaintext output from the CLI or MCP,
and there is no reveal API.

The current Flutter desktop editor keeps credentials masked and does not expose a
reveal control yet; the core still stores, imports and exports the preference so
the behaviour stays consistent across versions. This document records the
security model for when the control is present.

## When a reveal control is shown

- It is only available for saved passwords, key passphrases and inline private
  keys — never for file-based private keys, and only after the local preference
  is enabled.
- Each value opens in its own read-only preview. The replacement input stays
  masked and unchanged, so viewing cannot turn an old value into a replacement;
  a blank replacement still preserves the saved credential.
- Previews are discarded on hide, cancel, close, preference-off or a successful
  save. Reopening starts hidden even when the preference is on.
- Toggling a draft preference and viewing a value do not write the profile;
  saving persists the choice. The SQLite/keyring encryption paths remain
  responsible for storing the credential.

## Clipboard and memory

Revealing never copies automatically. Selecting and copying from a visible
preview puts plaintext on the user's clipboard, where clipboard managers may
retain it; hiding does not erase a user-owned clipboard. Preview entities and
their selection state are dropped on hide, but UI text buffers are not
guaranteed to be zeroized in memory. The ordinary masked replacement input
cannot copy its contents.

## Imports and exports

Portable exports contain the preference. Imports parse it but reset it to off
and report a fixed warning for a true value; the user can re-enable it locally if
desired. The preference never grants CLI or MCP plaintext-output permission.

Only synthetic credentials are used in the regression tests.
