/// Keychain — the SSH key credentials actually referenced by saved hosts.
///
/// The core owns key material, so this screen only shows metadata (file name,
/// algorithm, SHA256 fingerprint, the host that uses it) and can copy a key's
/// file name; it never receives the private key itself.
library;

import 'package:flutter/services.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';

typedef KeyLoader = Future<List<KeyInfo>> Function();
typedef KeyUpdater = Future<CoreWriteResult> Function({
  required String oldPath,
  required String newPath,
});
typedef KeyDeleter = Future<CoreWriteResult> Function(String path);

class KeychainScreen extends StatefulWidget {
  final KeyLoader? loadKeys;
  final KeyUpdater? updateKey;
  final KeyDeleter? deleteKey;

  const KeychainScreen({
    super.key,
    this.loadKeys,
    this.updateKey,
    this.deleteKey,
  });

  @override
  State<KeychainScreen> createState() => _KeychainScreenState();
}

class _KeychainScreenState extends State<KeychainScreen> {
  List<KeyInfo> _keys = const [];
  bool _loading = true;
  String? _error;
  bool _searching = false;
  String _query = '';
  final TextEditingController _search = TextEditingController();
  final TextEditingController _pathDraft = TextEditingController();
  String? _editingPath;
  String? _deletingPath;
  String? _notice;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void dispose() {
    _search.dispose();
    _pathDraft.dispose();
    super.dispose();
  }

  void _startEdit(KeyInfo key) {
    setState(() {
      _editingPath = key.fullPath;
      _deletingPath = null;
      _pathDraft.text = key.fullPath;
      _notice = null;
    });
  }

  Future<void> _saveEdit() async {
    final oldPath = _editingPath;
    final newPath = _pathDraft.text.trim();
    if (oldPath == null) return;
    if (newPath.isEmpty) {
      setState(() => _notice = 'Enter the private key path');
      return;
    }
    setState(() => _saving = true);
    try {
      final result = await (widget.updateKey ?? updateKeyInCore)(
        oldPath: oldPath,
        newPath: newPath,
      );
      if (!mounted) return;
      if (!result.ok) {
        setState(() {
          _saving = false;
          _notice = result.error ?? 'Could not update the key';
        });
        return;
      }
      setState(() {
        _saving = false;
        _editingPath = null;
        _notice = 'Key path updated';
      });
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _saving = false;
        _notice = '$error';
      });
    }
  }

  Future<void> _confirmDelete(String path) async {
    setState(() => _saving = true);
    try {
      final result = await (widget.deleteKey ?? deleteKeyFromCore)(path);
      if (!mounted) return;
      if (!result.ok) {
        setState(() {
          _saving = false;
          _notice = result.error ?? 'Could not remove the key';
        });
        return;
      }
      setState(() {
        _saving = false;
        _deletingPath = null;
        _notice = 'Key removed from its hosts';
      });
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _saving = false;
        _notice = '$error';
      });
    }
  }

  Future<void> _reload({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final keys = await (widget.loadKeys ?? loadKeysFromCore)();
      if (!mounted) return;
      setState(() {
        _keys = keys;
        _loading = false;
        _error = null;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _loading = false;
        _error = error is NativeBridgeError ? error.message : '$error';
      });
    }
  }

  List<KeyInfo> get _filtered {
    final query = _query.trim().toLowerCase();
    if (query.isEmpty) return _keys;
    return _keys
        .where(
          (k) =>
              k.title.toLowerCase().contains(query) ||
              k.sessionName.toLowerCase().contains(query) ||
              k.fingerprint.toLowerCase().contains(query),
        )
        .toList();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
          child: Row(
            children: [
              const Spacer(),
              Row(
                children: [
                  IconToggleButton(
                    icon: LucideIcons.search,
                    onPressed: () => setState(() {
                      _searching = !_searching;
                      if (!_searching) {
                        _search.clear();
                        _query = '';
                      }
                    }),
                  ),
                  const SizedBox(width: 6),
                  IconToggleButton(
                    icon: LucideIcons.refreshCw,
                    onPressed: () => _reload(),
                  ),
                ],
              ),
            ],
          ),
        ),
        if (_searching)
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 4),
            child: XtField(
              key: const ValueKey('keychain-search'),
              controller: _search,
              autofocus: true,
              onChanged: (value) => setState(() => _query = value),
              placeholder: AppLanguage.tr(context, 'Search keys'),
            ),
          ),
        SectionTitle(text: AppLanguage.tr(context, 'Keychain')),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 16),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 760),
              child: _body(theme),
            ),
          ),
        ),
      ],
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading) {
      return Padding(
        padding: const EdgeInsets.all(32),
        child: Center(
          child: Text(
            AppLanguage.tr(context, 'Loading keys…'),
            style: TextStyle(color: XtColors.textTertiary(context)),
          ),
        ),
      );
    }
    if (_error != null) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          children: [
            Text(
              AppLanguage.tr(context, _error!),
              textAlign: TextAlign.center,
              style: TextStyle(color: XtColors.danger(context)),
            ),
            const SizedBox(height: 12),
            XtActionButton(
              label: AppLanguage.tr(context, 'Retry'),
              variant: XtActionVariant.secondary,
              onPressed: () => _reload(),
            ),
          ],
        ),
      );
    }
    final keys = _filtered;
    if (keys.isEmpty) {
      return XtEmptyState(
        icon: LucideIcons.keyRound,
        title: AppLanguage.tr(
          context,
          _keys.isEmpty ? 'No SSH keys yet' : 'No matches',
        ),
        message: AppLanguage.tr(
          context,
          _keys.isEmpty
              ? 'Set a private key path on a host to see it here.'
              : 'Try a different search.',
        ),
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (_notice != null)
          Container(
            key: const ValueKey('keychain-notice'),
            margin: const EdgeInsets.only(bottom: 8),
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
            decoration: BoxDecoration(
              color: XtColors.selected(context),
              borderRadius: BorderRadius.circular(XtRadius.field),
            ),
            child: Text(
              AppLanguage.tr(context, _notice!),
              style: TextStyle(
                fontSize: 12,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
        for (final key in keys)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: _editingPath == key.fullPath && key.fullPath.isNotEmpty
                ? _KeyEditor(
                    key: ValueKey('key-editor-${key.sessionId}'),
                    controller: _pathDraft,
                    saving: _saving,
                    onSave: _saveEdit,
                    onCancel: () => setState(() => _editingPath = null),
                  )
                : _deletingPath == key.fullPath
                ? _KeyDeleteBar(
                    key: ValueKey('key-delete-bar-${key.sessionId}'),
                    label: _keyTitle(context, key),
                    busy: _saving,
                    onDelete: () => _confirmDelete(key.fullPath),
                    onCancel: () => setState(() => _deletingPath = null),
                  )
                : KeyCard(
                    keyInfo: key,
                    onEdit: key.fullPath.isEmpty ? null : () => _startEdit(key),
                    onDelete: key.fullPath.isEmpty
                        ? null
                        : () => setState(() {
                            _deletingPath = key.fullPath;
                            _editingPath = null;
                            _notice = null;
                          }),
                  ),
          ),
      ],
    );
  }
}

/// [KeyInfo.title] synthesises the two English labels below in the bridge; give
/// them a translation while leaving real file names untouched.
String _keyTitle(BuildContext context, KeyInfo key) {
  final title = key.title;
  if (title == 'Pasted key' || title == 'Key file') {
    return AppLanguage.tr(context, title);
  }
  return title;
}

class _KeyEditor extends StatelessWidget {
  final TextEditingController controller;
  final bool saving;
  final VoidCallback onSave;
  final VoidCallback onCancel;

  const _KeyEditor({
    super.key,
    required this.controller,
    required this.saving,
    required this.onSave,
    required this.onCancel,
  });

  @override
  Widget build(BuildContext context) {
    return XtCard(
      padding: const EdgeInsets.fromLTRB(14, 12, 10, 12),
      radius: XtRadius.cardLarge,
      child: Row(
        children: [
          Icon(
            LucideIcons.keyRound,
            size: 18,
            color: XtColors.textPrimary(context),
          ),
          const SizedBox(width: 12),
          Expanded(
            child: XtField(
              key: const ValueKey('key-path'),
              controller: controller,
              placeholder: AppLanguage.tr(context, 'Path to the private key'),
            ),
          ),
          const SizedBox(width: 8),
          XtActionButton(
            key: const ValueKey('key-save'),
            label: AppLanguage.tr(context, 'Save'),
            variant: XtActionVariant.primary,
            onPressed: saving ? null : onSave,
          ),
          const SizedBox(width: 6),
          XtActionButton(
            key: const ValueKey('key-cancel'),
            label: AppLanguage.tr(context, 'Cancel'),
            onPressed: saving ? null : onCancel,
          ),
        ],
      ),
    );
  }
}

class _KeyDeleteBar extends StatelessWidget {
  final String label;
  final bool busy;
  final VoidCallback onDelete;
  final VoidCallback onCancel;

  const _KeyDeleteBar({
    super.key,
    required this.label,
    required this.busy,
    required this.onDelete,
    required this.onCancel,
  });

  @override
  Widget build(BuildContext context) {
    return XtCard(
      padding: const EdgeInsets.fromLTRB(14, 12, 10, 12),
      radius: XtRadius.cardLarge,
      child: Row(
        children: [
          Icon(
            LucideIcons.triangleAlert,
            size: 16,
            color: XtColors.danger(context),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              AppLanguage.tr(
                context,
                'Remove “{name}” from its hosts? The key file stays on disk.',
                {'name': label},
              ),
              style: TextStyle(
                fontSize: 12.5,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
          XtActionButton(
            key: const ValueKey('key-delete-confirm'),
            label: AppLanguage.tr(context, 'Remove'),
            variant: XtActionVariant.danger,
            onPressed: busy ? null : onDelete,
          ),
          const SizedBox(width: 6),
          XtActionButton(
            label: AppLanguage.tr(context, 'Cancel'),
            onPressed: busy ? null : onCancel,
          ),
        ],
      ),
    );
  }
}

class KeyCard extends StatefulWidget {
  final KeyInfo keyInfo;
  final VoidCallback? onEdit;
  final VoidCallback? onDelete;

  const KeyCard({super.key, required this.keyInfo, this.onEdit, this.onDelete});

  @override
  State<KeyCard> createState() => _KeyCardState();
}

class _KeyCardState extends State<KeyCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final key = widget.keyInfo;
    final statusLine = switch (key.status) {
      'missing' => AppLanguage.tr(context, 'Key file missing'),
      'locked' => AppLanguage.tr(
        context,
        'Encrypted key · unlocked on connect',
      ),
      _ =>
        key.fingerprint.isEmpty
            ? AppLanguage.tr(context, 'Key')
            : key.fingerprint,
    };
    final statusColor = key.status == 'missing'
        ? XtColors.danger(context)
        : key.status == 'available'
        ? XtColors.textSecondary(context)
        : XtColors.textPrimary(context);
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: SizedBox(
        height: 76,
        child: XtCard(
          key: ValueKey('key-${key.sessionId}'),
          padding: const EdgeInsets.symmetric(horizontal: 14),
          radius: XtRadius.cardLarge,
          interactive: true,
          child: Row(
            children: [
              Container(
                width: 44,
                height: 44,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: XtColors.selected(context),
                  borderRadius: BorderRadius.circular(XtRadius.menu),
                ),
                child: Icon(
                  LucideIcons.keyRound,
                  size: 20,
                  color: XtColors.textPrimary(context),
                ),
              ),
              const SizedBox(width: 14),
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(
                            _keyTitle(context, key),
                            overflow: TextOverflow.ellipsis,
                            style: theme.typography.semiBold.copyWith(
                              fontSize: 15,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                        ),
                        if (key.keyType.isNotEmpty) ...[
                          const SizedBox(width: 8),
                          XtBadge(label: _algorithmLabel(key.keyType)),
                        ],
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      '$statusLine  ·  ${key.sessionName}',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12, color: statusColor),
                    ),
                  ],
                ),
              ),
              if (_hover && key.source == 'file')
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    if (widget.onEdit != null)
                      XtActionButton(
                        key: ValueKey('key-edit-${key.sessionId}'),
                        icon: LucideIcons.pencil,
                        tooltip: AppLanguage.tr(context, 'Edit key path'),
                        height: 30,
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        onPressed: widget.onEdit,
                      ),
                    if (widget.onDelete != null) ...[
                      const SizedBox(width: 4),
                      XtActionButton(
                        key: ValueKey('key-delete-${key.sessionId}'),
                        icon: LucideIcons.trash2,
                        variant: XtActionVariant.danger,
                        tooltip: AppLanguage.tr(
                          context,
                          'Remove key reference',
                        ),
                        height: 30,
                        padding: const EdgeInsets.symmetric(horizontal: 8),
                        onPressed: widget.onDelete,
                      ),
                    ],
                    const SizedBox(width: 4),
                    XtActionButton(
                      key: ValueKey('key-copy-${key.sessionId}'),
                      icon: LucideIcons.copy,
                      tooltip: AppLanguage.tr(context, 'Copy key file name'),
                      height: 30,
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      onPressed: () =>
                          Clipboard.setData(ClipboardData(text: key.fullPath)),
                    ),
                  ],
                ),
            ],
          ),
        ),
      ),
    );
  }

  static String _algorithmLabel(String type) {
    final lower = type.toLowerCase();
    if (lower.contains('ed25519')) return 'ED25519';
    if (lower.contains('rsa')) return 'RSA';
    if (lower.contains('ecdsa')) return 'ECDSA';
    if (lower.contains('ppk')) return 'PPK';
    return type.toUpperCase();
  }
}
