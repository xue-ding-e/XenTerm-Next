/// The host details drawer:
///
/// - 340px wide, animated over 200ms, on the page surface;
/// - a 56px panel header with the "Host Details" title and save / ⋯ / close
///   actions;
/// - body padding 15/15/85/15, 15px gaps and form cards with 10px between rows;
/// - an Address row made of the OS glyph tile next to the hostname field;
/// - a General card (label, parent group), a port row, credentials, encoding,
///   jump host and expect/send triggers;
/// - a Connect band at the bottom with the full-width primary action.
library;

import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../data/hosts.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import 'host_card.dart';
import 'host_form_widgets.dart';


class HostDetailsPanel extends StatefulWidget {
  final Host host;
  final VoidCallback onClose;
  final VoidCallback onConnect;

  /// Every saved host, for the jump-host selector and name-uniqueness checks.
  final List<Host> hosts;

  /// Persists the edited fields to the core; returns an error text or null.
  /// The credential arguments follow the core's keep-unless-touched contract:
  /// a blank password/key keeps the stored secret, the clear flags drop it.
  final Future<String?> Function(
    Host host,
    String password,
    bool clearPassword,
    String keyInline,
    bool clearKey,
  ) onSave;

  /// Deletes the session from the core; returns an error text or null.
  final Future<String?> Function(Host) onDelete;

  /// Offered only while creating (draft host): switching the connection type
  /// to serial reopens the drawer with the serial editor.
  final VoidCallback? onSwitchToSerial;

  /// Offered only while creating: switches the draft's protocol in place
  /// (ssh ↔ telnet) without rebuilding the editor.
  final ValueChanged<String>? onKindChanged;

  const HostDetailsPanel({
    super.key,
    required this.host,
    required this.onClose,
    required this.onConnect,
    required this.onSave,
    required this.onDelete,
    this.hosts = const [],
    this.onSwitchToSerial,
    this.onKindChanged,
  });

  @override
  State<HostDetailsPanel> createState() => _HostDetailsPanelState();
}

class _HostDetailsPanelState extends State<HostDetailsPanel> {
  /// A draft with no id yet: the type selector is offered and a successful
  /// save closes the drawer (nothing was persisted to come back to).
  bool get _creating => widget.host.id.isEmpty;

  /// Group names already in use — the Parent Group picker's options.
  List<String> get _existingGroups => {
    for (final host in widget.hosts)
      if (!host.ephemeral && host.group.trim().isNotEmpty)
        host.group.trim(),
  }.toList()..sort();

  late final TextEditingController _address = TextEditingController(
    text: widget.host.address,
  );
  late final TextEditingController _label = TextEditingController(
    text: widget.host.label,
  );
  late String _groupValue = widget.host.group;
  late final TextEditingController _port = TextEditingController(
    text: '${widget.host.port}',
  );
  late final TextEditingController _username = TextEditingController(
    text: widget.host.username,
  );
  final TextEditingController _password = TextEditingController();
  final TextEditingController _keyContent = TextEditingController();

  /// 'password' | 'key' — drives which credential fields render.
  late String _auth = widget.host.hasKey ? 'key' : 'password';
  bool _clearKey = false;
  bool _importingKey = false;
  late String _jumpId = widget.host.jumpHostId;
  late String _encoding = widget.host.encoding;
  late List<SessionTriggerRule> _triggers = List.of(widget.host.triggers);
  bool _saving = false;
  String? _saveError;

  @override
  void didUpdateWidget(HostDetailsPanel oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.host.id != widget.host.id) {
      _address.text = widget.host.address;
      _label.text = widget.host.label;
      _groupValue = widget.host.group;
      _port.text = '${widget.host.port}';
      _username.text = widget.host.username;
      _jumpId = widget.host.jumpHostId;
      _encoding = widget.host.encoding;
      _triggers = List.of(widget.host.triggers);
      _saveError = null;
    }
  }

  @override
  void dispose() {
    _address.dispose();
    _label.dispose();
    _port.dispose();
    _username.dispose();
    _password.dispose();
    _keyContent.dispose();
    super.dispose();
  }

  Host _edited() => Host(
    id: widget.host.id,
    label: _label.text.trim().isEmpty ? _address.text.trim() : _label.text,
    address: _address.text,
    port: int.tryParse(_port.text.trim()) ?? widget.host.port,
    username: _username.text,
    group: _groupValue,
    protocol: widget.host.protocol,
    os: widget.host.os,
    tags: widget.host.tags,
    lastUsed: widget.host.lastUsed,
    keyPath: _clearKey ? '' : widget.host.keyPath,
    hasKeyInline: _clearKey ? false : widget.host.hasKeyInline,
    auth: _auth,
    encoding: _encoding,
    baudRate: widget.host.baudRate,
    dataBits: widget.host.dataBits,
    stopBits: widget.host.stopBits,
    parity: widget.host.parity,
    flowControl: widget.host.flowControl,
    hasPassword: widget.host.hasPassword,
    jumpHostId: _jumpId,
    triggers: List.unmodifiable(_triggers),
  );

  /// The credential payload for [widget.onSave], applying the
  /// keep-unless-touched contract:
  /// - password auth: switching away from a stored key clears it;
  /// - key auth: pasted/imported material replaces, blank keeps, Clear drops.
  (String, bool, String, bool) _credentials() {
    if (_auth == 'password') {
      return (_password.text, false, '', widget.host.hasKey);
    }
    final clear = _clearKey;
    return ('', false, _keyContent.text, clear);
  }

  Future<void> _delete() async {
    final error = await widget.onDelete(widget.host);
    if (!mounted) return;
    if (error != null) setState(() => _saveError = error);
  }

  Future<void> _save() async {
    // Captured before the await: a successful create swaps widget.host for
    // the saved row, which would flip _creating to false mid-save.
    final creating = _creating;
    final edited = _edited();
    final validation = validateHostDraft(
      label: edited.label,
      address: edited.address,
      username: edited.username,
      kind: edited.protocol,
      hosts: widget.hosts,
      editingId: widget.host.id,
    );
    if (validation != null) {
      setState(() => _saveError = validation);
      return;
    }
    setState(() {
      _saving = true;
      _saveError = null;
    });
    final (password, clearPassword, keyInline, clearKey) = _credentials();
    final error = await widget.onSave(
      edited,
      password,
      clearPassword,
      keyInline,
      clearKey,
    );
    if (!mounted) return;
    setState(() {
      _saving = false;
      _saveError = error;
    });
    // Creating from a draft: a successful save has nothing left to edit.
    if (error == null && creating) widget.onClose();
  }

  /// Import key material from a file into the paste area (stored as inline
  /// content by the core — no path dependency on the user's machine). No
  /// extension filter: key files like `id_ed25519` have none.
  Future<void> _importKey() async {
    setState(() => _importingKey = true);
    try {
      final file = await openFile();
      if (file == null) return;
      final content = await File(file.path).readAsString();
      if (!mounted) return;
      setState(() {
        _keyContent.text = content;
        _clearKey = false;
      });
    } catch (error) {
      if (mounted) {
        setState(
          () => _saveError = AppLanguage.tr(
            context,
            'Could not read key: {error}',
            {'error': error},
          ),
        );
      }
    } finally {
      if (mounted) setState(() => _importingKey = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return ColoredBox(
      color: XtColors.page(context),
      child: Column(
        children: [
          HostDrawerHeader(
            title: AppLanguage.tr(context, 'Host Details'),
            actions: [
              // The check persists the drawer's edits into the core store.
              XtIconButton(
                icon: LucideIcons.check,
                size: 28,
                iconSize: 15,
                onPressed: _saving ? null : _save,
              ),
              const SizedBox(width: 4),
              HostRemoveMenu(onDelete: _delete),
              XtIconButton(
                icon: LucideIcons.arrowRightToLine,
                size: 28,
                iconSize: 16,
                onPressed: widget.onClose,
              ),
            ],
          ),
          Expanded(
            child: SingleChildScrollView(
              // formPaddings: 15px calc(15px - scrollbar) 85px 15px; gap 15.
              padding: const EdgeInsets.fromLTRB(15, 15, 15, 85),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  if (_saveError != null) ...[
                    Container(
                      padding: const EdgeInsets.all(10),
                      decoration: BoxDecoration(
                        color: XtColors.dangerTint(context),
                        borderRadius: BorderRadius.circular(XtRadius.card),
                      ),
                      child: Text(
                        AppLanguage.tr(context, _saveError!),
                        style: theme.typography.normal.copyWith(
                          fontSize: 12,
                          color: XtColors.dangerText(context),
                        ),
                      ),
                    ),
                    const SizedBox(height: 15),
                  ],
                  if (_creating && widget.onKindChanged != null) ...[
                    HostFormCard(
                      children: [
                        HostSectionTitle(
                          AppLanguage.tr(context, 'Connection type'),
                        ),
                        XtSelectField<String>(
                          key: const ValueKey('host-kind'),
                          value: widget.host.protocol,
                          options: const {
                            'ssh': 'SSH',
                            'telnet': 'Telnet',
                            'serial': 'Serial',
                          },
                          onChanged: (value) {
                            if (value == 'serial') {
                              widget.onSwitchToSerial?.call();
                            } else if (value != widget.host.protocol) {
                              widget.onKindChanged!(value);
                            }
                          },
                        ),
                      ],
                    ),
                    const SizedBox(height: 15),
                  ],
                  HostFormCard(
                    children: [
                      HostSectionTitle(AppLanguage.tr(context, 'Address')),
                      Row(
                        children: [
                          OsChipTile(host: widget.host, size: 36, iconSize: 20),
                          Expanded(
                            child: HostPanelField(
                              key: const ValueKey('host-address'),
                              controller: _address,
                              placeholder: AppLanguage.tr(
                                context,
                                'IP or Hostname',
                              ),
                            ),
                          ),
                        ],
                      ),
                    ],
                  ),
                  const SizedBox(height: 15),
                  HostFormCard(
                    children: [
                      HostSectionTitle(AppLanguage.tr(context, 'General')),
                      HostPanelField(
                        key: const ValueKey('label-field'),
                        controller: _label,
                        placeholder: AppLanguage.tr(context, 'Label'),
                      ),
                      XtSelectField<String>(
                        key: const ValueKey('host-group'),
                        value: _groupValue,
                        options: {
                          '': 'None',
                          for (final group in _existingGroups) group: group,
                        },
                        onChanged: (value) =>
                            setState(() => _groupValue = value),
                      ),
                    ],
                  ),
                  const SizedBox(height: 15),
                  HostFormCard(
                    padding: const EdgeInsets.fromLTRB(20, 15, 20, 15),
                    children: [
                      _PortRow(
                        controller: _port,
                        label: AppLanguage.tr(
                          context,
                          widget.host.protocol == 'telnet'
                              ? 'Telnet on'
                              : 'SSH on',
                        ),
                      ),
                      Column(
                        crossAxisAlignment: CrossAxisAlignment.stretch,
                        children: [
                          // Telnet dials host:port only — the core's telnet
                          // session ignores credentials, so offering them
                          // would be a lie.
                          if (widget.host.protocol == 'ssh') ...[
                            HostSectionTitle(
                              AppLanguage.tr(context, 'Credentials'),
                            ),
                            const SizedBox(height: 10),
                            HostPanelField(
                              key: const ValueKey('host-username'),
                              controller: _username,
                              placeholder: AppLanguage.tr(context, 'Username'),
                              leading: LucideIcons.user,
                            ),
                            const SizedBox(height: 12),
                          HostSectionTitle(
                            AppLanguage.tr(context, 'Authentication'),
                          ),
                          const SizedBox(height: 6),
                          XtSelectField<String>(
                            key: const ValueKey('host-auth'),
                            value: _auth,
                            options: const {
                              'password': 'Password',
                              'key': 'Private key',
                            },
                            onChanged: (value) => setState(() {
                              _auth = value;
                              _clearKey = false;
                            }),
                          ),
                          if (_auth == 'password') ...[
                            const SizedBox(height: 10),
                            HostPanelField(
                              key: const ValueKey('host-password'),
                              controller: _password,
                              placeholder: widget.host.hasPassword
                                  ? AppLanguage.tr(
                                      context,
                                      'Saved — leave blank to keep',
                                    )
                                  : AppLanguage.tr(context, 'Password'),
                              leading: LucideIcons.lock,
                              obscure: true,
                            ),
                          ] else ...[
                            const SizedBox(height: 10),
                            Row(
                              children: [
                                Expanded(
                                  child: XtActionButton(
                                    key: const ValueKey('host-key-import'),
                                    icon: _importingKey
                                        ? LucideIcons.loaderCircle
                                        : LucideIcons.fileUp,
                                    label: _importingKey
                                        ? AppLanguage.tr(
                                            context,
                                            'Importing…',
                                          )
                                        : AppLanguage.tr(
                                            context,
                                            'Import key file…',
                                          ),
                                    variant: XtActionVariant.secondary,
                                    height: 34,
                                    expand: true,
                                    onPressed: _importingKey ? null : _importKey,
                                  ),
                                ),
                                if (widget.host.hasKey &&
                                    _keyContent.text.trim().isEmpty &&
                                    !_clearKey) ...[
                                  const SizedBox(width: 8),
                                  XtActionButton(
                                    key: const ValueKey('host-key-clear'),
                                    icon: LucideIcons.trash2,
                                    label: AppLanguage.tr(context, 'Clear'),
                                    variant: XtActionVariant.danger,
                                    height: 34,
                                    onPressed: () =>
                                        setState(() => _clearKey = true),
                                  ),
                                ],
                              ],
                            ),
                            const SizedBox(height: 8),
                            if (_clearKey)
                              Padding(
                                padding: const EdgeInsets.only(bottom: 8),
                                child: Text(
                                  AppLanguage.tr(
                                    context,
                                    'The stored key will be removed on save.',
                                  ),
                                  key: const ValueKey('host-key-clear-note'),
                                  style: TextStyle(
                                    fontSize: 11,
                                    color: XtColors.textTertiary(context),
                                  ),
                                ),
                              )
                            else if (widget.host.hasKey &&
                                _keyContent.text.trim().isEmpty)
                              Padding(
                                padding: const EdgeInsets.only(bottom: 8),
                                child: Text(
                                  AppLanguage.tr(
                                    context,
                                    'A key is stored — import or paste below to '
                                    'replace it.',
                                  ),
                                  key: const ValueKey('host-key-stored'),
                                  style: TextStyle(
                                    fontSize: 11,
                                    color: XtColors.textTertiary(context),
                                  ),
                                ),
                              ),
                            XtField(
                              key: const ValueKey('host-key-content'),
                              controller: _keyContent,
                              height: 110,
                              maxLines: 8,
                              minLines: 4,
                              placeholder: AppLanguage.tr(
                                context,
                                '…or paste the private key content here',
                              ),
                              onChanged: (_) => setState(() {}),
                            ),
                          ],
                          ],
                          if (widget.host.protocol == 'ssh' &&
                              widget.hosts.any(
                                (h) =>
                                    h.protocol == 'ssh' &&
                                    h.id != widget.host.id &&
                                    !h.ephemeral,
                              )) ...[
                            const SizedBox(height: 14),
                            XtFieldLabel(
                              AppLanguage.tr(context, 'Jump host'),
                            ),
                            XtSelectField<String>(
                              key: const ValueKey('host-jump'),
                              value: _jumpId,
                              options: {
                                '': 'None',
                                for (final candidate in widget.hosts)
                                  if (candidate.protocol == 'ssh' &&
                                      candidate.id != widget.host.id &&
                                      !candidate.ephemeral)
                                    candidate.id: candidate.label,
                              },
                              onChanged: (value) =>
                                  setState(() => _jumpId = value),
                            ),
                          ],
                          const SizedBox(height: 14),
                          HostSectionTitle(AppLanguage.tr(context, 'Encoding')),
                          const SizedBox(height: 6),
                          XtSelectField<String>(
                            key: const ValueKey('host-encoding'),
                            value: _encoding,
                            options: const {
                              'utf-8': 'UTF-8 (auto)',
                              'gbk': 'GBK / 简体中文',
                              'gb18030': 'GB18030',
                              'big5': 'Big5 / 繁体中文',
                              'shift_jis': 'Shift_JIS / 日语',
                              'euc-kr': 'EUC-KR / 韩语',
                              'windows-1251': 'Windows-1251 / 西里尔',
                              'iso-8859-1': 'Latin-1',
                            },
                            onChanged: (value) =>
                                setState(() => _encoding = value),
                          ),
                        ],
                      ),
                    ],
                  ),
                  const SizedBox(height: 15),
                  HostFormCard(
                    padding: const EdgeInsets.fromLTRB(20, 15, 20, 15),
                    children: [
                      HostSectionTitle(AppLanguage.tr(context, 'Triggers')),
                      const SizedBox(height: 4),
                      Text(
                        AppLanguage.tr(
                          context,
                          'Wait for a prompt, answer automatically. Responses are '
                          'stored securely; a blank field keeps the saved value.',
                        ),
                        style: TextStyle(
                          fontSize: 12,
                          color: XtColors.textTertiary(context),
                        ),
                      ),
                      if (_triggers.isEmpty)
                        Padding(
                          padding: const EdgeInsets.only(top: 10),
                          child: Text(
                            AppLanguage.tr(context, 'No triggers yet.'),
                            key: const ValueKey('host-triggers-empty'),
                            style: TextStyle(
                              fontSize: 12,
                              color: XtColors.textTertiary(context),
                            ),
                          ),
                        )
                      else
                        for (var i = 0; i < _triggers.length; i++)
                          _TriggerRow(
                            key: ValueKey('host-trigger-$i'),
                            rule: _triggers[i],
                            onChanged: (rule) => setState(() {
                              _triggers[i] = rule;
                            }),
                            onRemoved: () => setState(() {
                              _triggers.removeAt(i);
                            }),
                          ),
                      const SizedBox(height: 10),
                      XtActionButton(
                        key: const ValueKey('host-trigger-add'),
                        icon: LucideIcons.plus,
                        label: AppLanguage.tr(context, 'Add trigger'),
                        expand: true,
                        onPressed: () => setState(() {
                          _triggers.add(const SessionTriggerRule(expect: ''));
                        }),
                      ),
                    ],
                  ),
                ],
              ),
            ),
          ),
          // The Connect band: page surface, 20/15 padding, full-width primary.
          Container(
            color: XtColors.page(context),
            padding: const EdgeInsets.symmetric(horizontal: 15, vertical: 20),
            child: XtActionButton(
              label: AppLanguage.tr(context, 'Connect'),
              variant: XtActionVariant.primary,
              height: XtMetrics.controlHeight,
              expand: true,
              onPressed: widget.onConnect,
            ),
          ),
        ],
      ),
    );
  }
}

/// "SSH on [22] port" over a hairline divider.
class _PortRow extends StatelessWidget {
  final TextEditingController controller;
  final String label;

  const _PortRow({required this.controller, required this.label});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(bottom: 10),
          child: Row(
            children: [
              Text(
                label,
                style: theme.typography.normal.copyWith(
                  fontSize: 16,
                  color: XtColors.textPrimary(context),
                ),
              ),
              const SizedBox(width: 10),
              SizedBox(
                width: 79,
                child: HostPanelField(
                  controller: controller,
                  placeholder: '22',
                  height: 28,
                ),
              ),
              const SizedBox(width: 10),
              Text(
                AppLanguage.tr(context, 'port'),
                style: theme.typography.normal.copyWith(
                  fontSize: 16,
                  color: XtColors.textPrimary(context),
                ),
              ),
            ],
          ),
        ),
        const XtDivider(),
      ],
    );
  }
}

/// One expect/send rule inside the host drawer. The response field stays
/// blank-looking while a secret is stored ("Saved…"): only a typed value
/// replaces it, matching the password field's semantics.
class _TriggerRow extends StatefulWidget {
  final SessionTriggerRule rule;
  final ValueChanged<SessionTriggerRule> onChanged;
  final VoidCallback onRemoved;

  const _TriggerRow({
    super.key,
    required this.rule,
    required this.onChanged,
    required this.onRemoved,
  });

  @override
  State<_TriggerRow> createState() => _TriggerRowState();
}

class _TriggerRowState extends State<_TriggerRow> {
  late final TextEditingController _expect = TextEditingController(
    text: widget.rule.expect,
  );
  late final TextEditingController _response = TextEditingController(
    text: widget.rule.draftResponse,
  );

  @override
  void initState() {
    super.initState();
    // Typing must reach the parent's save payload immediately; the row keeps
    // no separate draft state.
    _expect.addListener(() => _push(expect: _expect.text));
    _response.addListener(() => _push(response: _response.text));
  }

  @override
  void dispose() {
    _expect.dispose();
    _response.dispose();
    super.dispose();
  }

  void _push({String? expect, String? response, bool? appendEnter, bool? repeat, bool? clear}) {
    widget.onChanged(
      widget.rule.copyWith(
        expect: expect,
        draftResponse: response,
        appendEnter: appendEnter,
        repeat: repeat,
        clearResponse: clear,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final kept =
        widget.rule.hasResponse &&
        widget.rule.draftResponse.isEmpty &&
        !widget.rule.clearResponse;
    return XtCard(
      margin: const EdgeInsets.only(top: 10),
      padding: const EdgeInsets.all(10),
      radius: XtRadius.card,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          HostPanelField(
            key: const ValueKey('trigger-expect'),
            controller: _expect,
            placeholder: AppLanguage.tr(context, 'Wait for (e.g. login:)'),
            leading: LucideIcons.eye,
            height: 30,
          ),
          const SizedBox(height: 8),
          HostPanelField(
            key: const ValueKey('trigger-response'),
            controller: _response,
            placeholder: kept
                ? AppLanguage.tr(context, 'Saved — leave blank to keep')
                : AppLanguage.tr(context, 'Response'),
            leading: LucideIcons.messageSquare,
            height: 30,
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    XtSwitch(
                      value: widget.rule.appendEnter,
                      onChanged: (value) => _push(appendEnter: value),
                    ),
                    const SizedBox(width: 6),
                    Text(
                      AppLanguage.tr(context, 'Enter'),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.textTertiary(context),
                      ),
                    ),
                  ],
                ),
              ),
              Expanded(
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    XtSwitch(
                      value: widget.rule.repeat,
                      onChanged: (value) => _push(repeat: value),
                    ),
                    const SizedBox(width: 6),
                    Text(
                      AppLanguage.tr(context, 'Repeat'),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.textTertiary(context),
                      ),
                    ),
                  ],
                ),
              ),
              XtIconButton(
                key: const ValueKey('trigger-remove'),
                icon: LucideIcons.trash2,
                size: 28,
                iconSize: 14,
                onPressed: widget.onRemoved,
              ),
            ],
          ),
        ],
      ),
    );
  }
}
