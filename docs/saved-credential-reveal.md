# Viewing saved credentials in the desktop editor

Saved sessions have an **Allow viewing saved credentials** preference, off by
default. Existing profiles without `allow_secret_reveal` also default to off.
Checking it only grants local GUI permission; it does not display a secret.

After opting in, click **Show saved value** beside the saved password/key
passphrase or an existing inline private key. Each value opens in its own
read-only preview. Replacement password input stays masked and unchanged, so
viewing cannot accidentally turn an old value into a replacement. A blank
replacement still preserves the saved credential. File-based private keys are
never opened or read by this feature.

Click **Hide**, turn the preference off, cancel, close, or successfully save to
discard previews. Reopening starts hidden, even when the preference is on.
Failed saves retain edits for retry. Viewing and toggling a draft preference do
not write the profile; **Save** persists the choice. The existing SQLite/keyring
encryption paths remain responsible for storing credentials.

Revealing never copies automatically. Users may explicitly select and copy from
a visible preview; doing so puts plaintext on their system clipboard and may
make it available to clipboard managers. Hiding does not erase a user-owned
clipboard. Preview entities and their selection/history are discarded on hide;
GPUI's text buffers are not guaranteed to be zeroized in memory. The ordinary
masked replacement input cannot copy its contents.

Imports parse the compatibility preference but reset it to off and report a
fixed warning for a true value. Re-enable locally if desired. This preference
never grants CLI or MCP plaintext-output permission and adds no reveal API.
Only synthetic credentials are used in the regression tests.
