/// Snippets — the core's saved quick commands, with real create/edit/delete
/// and one-click execution in the active terminal. No fixture data: the list
/// loads from `ConfigStore` through the FFI bridge and every action writes back
/// to `sessions.db`.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';

typedef SnippetLoader = Future<List<QuickCommand>> Function();
typedef SnippetSaver = Future<CoreWriteResult> Function({
  required int index,
  required String name,
  required String command,
  required String group,
  required bool sendEnter,
});
typedef SnippetDeleter = Future<CoreWriteResult> Function(int index);

class SnippetsScreen extends StatefulWidget {
  final SnippetLoader? loadSnippets;
  final SnippetSaver? saveSnippet;
  final SnippetDeleter? deleteSnippet;

  /// Sends the snippet to the active terminal (RootShell wires this).

  const SnippetsScreen({
    super.key,
    this.loadSnippets,
    this.saveSnippet,
    this.deleteSnippet,
  });

  @override
  State<SnippetsScreen> createState() => _SnippetsScreenState();
}

class _SnippetsScreenState extends State<SnippetsScreen> {
  List<QuickCommand> _snippets = const [];
  bool _loading = true;
  String? _error;
  String? _notice;
  String _query = '';
  bool _searching = false;
  QuickCommand? _editing; // null with _drawerOpen => new snippet
  bool _drawerOpen = false;
  final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    _reload();
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  Future<void> _reload({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final snippets =
          await (widget.loadSnippets ?? loadQuickCommandsFromCore)();
      if (!mounted) return;
      setState(() {
        _snippets = snippets;
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

  List<QuickCommand> get _filtered {
    final query = _query.trim().toLowerCase();
    if (query.isEmpty) return _snippets;
    return _snippets
        .where(
          (s) =>
              s.name.toLowerCase().contains(query) ||
              s.command.toLowerCase().contains(query) ||
              s.group.toLowerCase().contains(query),
        )
        .toList();
  }

  Future<String?> _save({
    required int index,
    required String name,
    required String command,
    required String group,
    required bool sendEnter,
  }) async {
    try {
      final result = await (widget.saveSnippet ?? saveQuickCommandToCore)(
        index: index,
        name: name,
        command: command,
        group: group,
        sendEnter: sendEnter,
      );
      if (!result.ok) return result.error ?? 'Save failed';
      await _reload(silent: true);
      return null;
    } catch (error) {
      return '$error';
    }
  }

  Future<String?> _delete(QuickCommand snippet) async {
    try {
      final result = await (widget.deleteSnippet ?? deleteQuickCommandFromCore)(
        snippet.index,
      );
      if (!result.ok) return result.error ?? 'Delete failed';
      _closeDrawer();
      await _reload(silent: true);
      return null;
    } catch (error) {
      return '$error';
    }
  }

  void _openNew() {
    setState(() {
      _editing = null;
      _drawerOpen = true;
    });
  }

  void _openEdit(QuickCommand snippet) {
    setState(() {
      _editing = snippet;
      _drawerOpen = true;
    });
  }

  void _closeDrawer() {
    setState(() {
      _drawerOpen = false;
      _editing = null;
    });
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Padding(
                padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
                child: Row(
                  children: [
                    SplitButton(
                      label: AppLanguage.tr(context, 'New snippet'),
                      icon: LucideIcons.plus,
                      onPressed: _openNew,
                    ),
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
                    key: const ValueKey('snippet-search'),
                    controller: _search,
                    autofocus: true,
                    onChanged: (value) => setState(() => _query = value),
                    placeholder: AppLanguage.tr(context, 'Search snippets'),
                  ),
                ),
              if (_notice != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
                  child: Container(
                    padding: const EdgeInsets.symmetric(
                      horizontal: 10,
                      vertical: 8,
                    ),
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
                ),
              SectionTitle(text: AppLanguage.tr(context, 'Snippets')),
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
          ),
        ),
        ClipRect(
          child: AnimatedContainer(
            duration: XtMotion.micro,
            curve: XtMotion.easeOut,
            width: _drawerOpen ? 340 : 0,
            child: _drawerOpen
                ? OverflowBox(
                    alignment: Alignment.centerRight,
                    minWidth: 340,
                    maxWidth: 340,
                    child: SnippetEditor(
                      key: ValueKey(_editing?.index ?? -1),
                      snippet: _editing,
                      onSave: _save,
                      onDelete: _editing == null ? null : _delete,
                      onClose: _closeDrawer,
                    ),
                  )
                : const SizedBox.shrink(),
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
            AppLanguage.tr(context, 'Loading snippets…'),
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
    final snippets = _filtered;
    if (snippets.isEmpty) {
      return XtEmptyState(
        icon: LucideIcons.braces,
        title: AppLanguage.tr(
          context,
          _snippets.isEmpty ? 'No snippets yet' : 'No matches',
        ),
        message: AppLanguage.tr(
          context,
          _snippets.isEmpty
              ? 'Save commands here; the connected terminal sidebar runs them.'
              : 'Try a different search.',
        ),
      );
    }
    return Column(
      children: [
        for (final snippet in snippets)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: _SnippetCard(
              snippet: snippet,
              onEdit: () => _openEdit(snippet),
              onDelete: () => _delete(snippet),
            ),
          ),
      ],
    );
  }
}

class _SnippetCard extends StatefulWidget {
  final QuickCommand snippet;
  final VoidCallback onEdit;
  final VoidCallback onDelete;

  const _SnippetCard({
    required this.snippet,
    required this.onEdit,
    required this.onDelete,
  });

  @override
  State<_SnippetCard> createState() => _SnippetCardState();
}

class _SnippetCardState extends State<_SnippetCard> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: SizedBox(
        height: 76,
        child: XtCard(
          key: ValueKey('snippet-${widget.snippet.index}'),
          padding: const EdgeInsets.symmetric(horizontal: 14),
          radius: XtRadius.cardLarge,
          onTap: widget.onEdit,
          child: Row(
            children: [
              Container(
                width: 44,
                height: 44,
                alignment: Alignment.center,
                decoration: BoxDecoration(
                  color: XtColors.sunken(context),
                  borderRadius: BorderRadius.circular(XtRadius.menu),
                ),
                child: Icon(
                  LucideIcons.terminal,
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
                            widget.snippet.name,
                            overflow: TextOverflow.ellipsis,
                            style: theme.typography.semiBold.copyWith(
                              fontSize: 15,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                        ),
                        if (widget.snippet.group.isNotEmpty) ...[
                          const SizedBox(width: 8),
                          XtBadge(label: widget.snippet.group),
                        ],
                      ],
                    ),
                    const SizedBox(height: 3),
                    Text(
                      widget.snippet.command,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontFamily: 'Consolas',
                        fontFamilyFallback: const ['Menlo', 'Courier New'],
                        fontSize: 12.5,
                        color: XtColors.textSecondary(context),
                      ),
                    ),
                  ],
                ),
              ),
              if (_hover) ...[
                XtActionButton(
                  icon: LucideIcons.play,
                  tooltip: AppLanguage.tr(context, 'Run'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: widget.onEdit,
                ),
                const SizedBox(width: 4),
                XtActionButton(
                  icon: LucideIcons.pencil,
                  tooltip: AppLanguage.tr(context, 'Edit'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: widget.onEdit,
                ),
                const SizedBox(width: 4),
                XtActionButton(
                  icon: LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  tooltip: AppLanguage.tr(context, 'Delete'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: widget.onDelete,
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// The 340px editor drawer: name, group, command, send-enter, save/delete.
class SnippetEditor extends StatefulWidget {
  final QuickCommand? snippet;
  final Future<String?> Function({
    required int index,
    required String name,
    required String command,
    required String group,
    required bool sendEnter,
  })
  onSave;
  final Future<String?> Function(QuickCommand snippet)? onDelete;
  final VoidCallback onClose;

  const SnippetEditor({
    super.key,
    required this.snippet,
    required this.onSave,
    required this.onDelete,
    required this.onClose,
  });

  @override
  State<SnippetEditor> createState() => _SnippetEditorState();
}

class _SnippetEditorState extends State<SnippetEditor> {
  late final TextEditingController _name = TextEditingController(
    text: widget.snippet?.name ?? '',
  );
  late final TextEditingController _group = TextEditingController(
    text: widget.snippet?.group ?? '',
  );
  late final TextEditingController _command = TextEditingController(
    text: widget.snippet?.command ?? '',
  );
  late bool _sendEnter = widget.snippet?.sendEnter ?? true;
  bool _saving = false;
  bool _deleting = false;
  String? _error;

  @override
  void dispose() {
    _name.dispose();
    _group.dispose();
    _command.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    if (_command.text.trim().isEmpty) {
      setState(() => _error = 'The command cannot be empty.');
      return;
    }
    setState(() {
      _saving = true;
      _error = null;
    });
    final error = await widget.onSave(
      index: widget.snippet?.index ?? -1,
      name: _name.text.trim(),
      command: _command.text.trim(),
      group: _group.text.trim(),
      sendEnter: _sendEnter,
    );
    if (!mounted) return;
    setState(() => _saving = false);
    if (error != null) {
      setState(() => _error = error);
    } else {
      widget.onClose();
    }
  }

  Future<void> _delete() async {
    final delete = widget.onDelete;
    if (delete == null) return;
    setState(() {
      _deleting = true;
      _error = null;
    });
    final error = await delete(widget.snippet!);
    if (!mounted) return;
    setState(() => _deleting = false);
    if (error != null) setState(() => _error = error);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return XtCard(
      margin: const EdgeInsets.fromLTRB(0, 8, 8, 8),
      padding: EdgeInsets.zero,
      border: false,
      radius: XtRadius.card,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          XtPanelHeader(
            title: AppLanguage.tr(
              context,
              widget.snippet == null ? 'New snippet' : 'Snippet',
            ),
            height: 56,
            padding: const EdgeInsets.symmetric(horizontal: 15),
            actions: [
              if (widget.snippet != null)
                XtActionButton(
                  key: const ValueKey('snippet-delete'),
                  icon: _deleting
                      ? LucideIcons.loaderCircle
                      : LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  height: 28,
                  padding: const EdgeInsets.symmetric(horizontal: 7),
                  tooltip: AppLanguage.tr(context, 'Delete snippet'),
                  onPressed: _deleting ? null : _delete,
                ),
              const SizedBox(width: 6),
              XtActionButton(
                key: const ValueKey('snippet-save'),
                icon: _saving ? LucideIcons.loaderCircle : LucideIcons.check,
                variant: XtActionVariant.secondary,
                height: 28,
                padding: const EdgeInsets.symmetric(horizontal: 7),
                tooltip: AppLanguage.tr(context, 'Save snippet'),
                onPressed: _saving ? null : _save,
              ),
              const SizedBox(width: 6),
              XtActionButton(
                icon: LucideIcons.x,
                height: 28,
                padding: const EdgeInsets.symmetric(horizontal: 7),
                tooltip: AppLanguage.tr(context, 'Close'),
                onPressed: widget.onClose,
              ),
            ],
          ),
          Expanded(
            child: ListView(
              padding: const EdgeInsets.fromLTRB(15, 15, 15, 15),
              children: [
                if (_error != null) ...[
                  Container(
                    padding: const EdgeInsets.all(10),
                    decoration: BoxDecoration(
                      color: XtColors.dangerTint(context),
                      borderRadius: BorderRadius.circular(XtRadius.field),
                    ),
                    child: Text(
                      AppLanguage.tr(context, _error!),
                      style: TextStyle(
                        fontSize: 12,
                        color: XtColors.dangerText(context),
                      ),
                    ),
                  ),
                  const SizedBox(height: 12),
                ],
                XtField(
                  key: const ValueKey('snippet-name'),
                  controller: _name,
                  autofocus: true,
                  placeholder: AppLanguage.tr(context, 'Name'),
                ),
                const SizedBox(height: 10),
                XtField(
                  key: const ValueKey('snippet-group'),
                  controller: _group,
                  placeholder: AppLanguage.tr(context, 'Group (optional)'),
                ),
                const SizedBox(height: 10),
                XtField(
                  key: const ValueKey('snippet-command'),
                  controller: _command,
                  placeholder: AppLanguage.tr(context, 'Command'),
                  maxLines: 8,
                  minLines: 3,
                ),
                const SizedBox(height: 14),
                Row(
                  children: [
                    Expanded(
                      child: Text(
                        AppLanguage.tr(context, 'Press Enter after running'),
                        style: theme.typography.normal.copyWith(
                          fontSize: 13,
                          color: XtColors.textPrimary(context),
                        ),
                      ),
                    ),
                    XtSwitch(
                      key: const ValueKey('snippet-send-enter'),
                      value: _sendEnter,
                      onChanged: (value) => setState(() => _sendEnter = value),
                    ),
                  ],
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}
