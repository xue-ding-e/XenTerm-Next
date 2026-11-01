/// Logs — real session transcripts recorded by the core when "Save session
/// logs" is enabled in Settings. Rows open a read-only viewer; deletion removes
/// the JSONL file. No plan/account content exists in this product.
library;

import 'package:flutter/services.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import '../../widgets/toolbar.dart';

typedef LogLoader = Future<List<SessionLogEntry>> Function();
typedef LogReader = Future<SessionLogContent> Function(String file);
typedef LogDeleter = Future<CoreWriteResult> Function(String file);

class LogsScreen extends StatefulWidget {
  final LogLoader? loadLogs;
  final LogReader? readLog;
  final LogDeleter? deleteLog;

  const LogsScreen({super.key, this.loadLogs, this.readLog, this.deleteLog});

  @override
  State<LogsScreen> createState() => _LogsScreenState();
}

class _LogsScreenState extends State<LogsScreen> {
  List<SessionLogEntry> _logs = const [];
  bool _loading = true;
  String? _error;
  SessionLogEntry? _open;
  SessionLogContent? _content;
  bool _reading = false;
  bool _confirmClear = false;

  @override
  void initState() {
    super.initState();
    _reload();
  }

  Future<void> _reload({bool silent = false}) async {
    if (!silent) {
      setState(() {
        _loading = true;
        _error = null;
      });
    }
    try {
      final logs = await (widget.loadLogs ?? loadSessionLogsFromCore)();
      if (!mounted) return;
      setState(() {
        _logs = logs;
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

  Future<void> _openLog(SessionLogEntry entry) async {
    setState(() {
      _open = entry;
      _content = null;
      _reading = true;
    });
    try {
      final content = await (widget.readLog ?? readSessionLogFromCore)(
        entry.file,
      );
      if (!mounted || _open?.file != entry.file) return;
      setState(() {
        _content = content;
        _reading = false;
      });
    } catch (error) {
      if (!mounted) return;
      setState(() {
        _reading = false;
        _error = error is NativeBridgeError ? error.message : '$error';
      });
    }
  }

  Future<void> _delete(SessionLogEntry entry) async {
    try {
      final result = await (widget.deleteLog ?? deleteSessionLogFromCore)(
        entry.file,
      );
      if (!result.ok) {
        if (!mounted) return;
        setState(() => _error = result.error ?? 'Could not delete the log');
        return;
      }
      if (_open?.file == entry.file) {
        setState(() {
          _open = null;
          _content = null;
        });
      }
      await _reload(silent: true);
    } catch (error) {
      if (!mounted) return;
      setState(() => _error = '$error');
    }
  }

  Future<void> _deleteAll() async {
    final entries = [..._logs];
    for (final entry in entries) {
      try {
        await (widget.deleteLog ?? deleteSessionLogFromCore)(entry.file);
      } catch (_) {
        // Keep deleting the rest; the reload below surfaces what is left.
      }
    }
    if (!mounted) return;
    setState(() {
      _confirmClear = false;
      _open = null;
      _content = null;
    });
    await _reload(silent: true);
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return LayoutBuilder(
      builder: (context, constraints) {
        // The viewer takes at most 560px and always leaves the list room to
        // breathe, so a 940px-wide window keeps both panes usable.
        final viewerWidth = (constraints.maxWidth - 420).clamp(320.0, 560.0);
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(34, 18, 34, 0),
                    child: Row(
                      children: [
                        Expanded(
                          child: Row(
                            children: [
                              Flexible(
                                child: Text(
                                  AppLanguage.tr(context, 'Session logs'),
                                  maxLines: 1,
                                  overflow: TextOverflow.ellipsis,
                                  style: theme.typography.semiBold.copyWith(
                                    fontSize: 16,
                                    color: XtColors.textPrimary(context),
                                  ),
                                ),
                              ),
                              const SizedBox(width: 8),
                              Text(
                                '${_logs.length}',
                                style: TextStyle(
                                  fontSize: 12,
                                  color: XtColors.textTertiary(context),
                                ),
                              ),
                            ],
                          ),
                        ),
                        IconToggleButton(
                          icon: LucideIcons.refreshCw,
                          onPressed: () => _reload(),
                        ),
                        if (_logs.isNotEmpty) ...[
                          const SizedBox(width: 6),
                          IconToggleButton(
                            key: const ValueKey('logs-clear'),
                            icon: LucideIcons.trash2,
                            onPressed: () =>
                                setState(() => _confirmClear = !_confirmClear),
                          ),
                        ],
                      ],
                    ),
                  ),
                  if (_confirmClear)
                    Container(
                      margin: const EdgeInsets.fromLTRB(34, 10, 34, 0),
                      padding: const EdgeInsets.fromLTRB(14, 10, 10, 10),
                      decoration: BoxDecoration(
                        color: XtColors.dangerTint(context),
                        borderRadius: BorderRadius.circular(XtRadius.field),
                      ),
                      child: Row(
                        children: [
                          Icon(
                            LucideIcons.triangleAlert,
                            size: 16,
                            color: XtColors.dangerText(context),
                          ),
                          const SizedBox(width: 8),
                          Expanded(
                            child: Text(
                              _logs.length == 1
                                  ? AppLanguage.tr(
                                      context,
                                      'Delete the only session log? This cannot be undone.',
                                    )
                                  : AppLanguage.tr(
                                      context,
                                      'Delete all {count} session logs? This cannot be undone.',
                                      {'count': _logs.length},
                                    ),
                              style: TextStyle(
                                fontSize: 13,
                                color: XtColors.textPrimary(context),
                              ),
                            ),
                          ),
                          XtActionButton(
                            key: const ValueKey('logs-clear-confirm'),
                            label: AppLanguage.tr(context, 'Delete all'),
                            variant: XtActionVariant.danger,
                            onPressed: _deleteAll,
                          ),
                          const SizedBox(width: 6),
                          XtActionButton(
                            label: AppLanguage.tr(context, 'Cancel'),
                            onPressed: () =>
                                setState(() => _confirmClear = false),
                          ),
                        ],
                      ),
                    ),
                  const Padding(
                    padding: EdgeInsets.symmetric(horizontal: 34),
                    child: XtDivider(),
                  ),
                  Expanded(child: _body(theme)),
                ],
              ),
            ),
            if (_open != null)
              SizedBox(
                width: viewerWidth,
                child: _LogViewer(
                  entry: _open!,
                  content: _content,
                  reading: _reading,
                  onDelete: () => _delete(_open!),
                  onClose: () => setState(() {
                    _open = null;
                    _content = null;
                  }),
                ),
              ),
          ],
        );
      },
    );
  }

  Widget _body(ThemeData theme) {
    if (_loading) {
      return Center(
        child: Text(
          AppLanguage.tr(context, 'Loading session logs…'),
          style: TextStyle(color: XtColors.textTertiary(context)),
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
    if (_logs.isEmpty) {
      return XtEmptyState(
        icon: LucideIcons.clipboardList,
        title: AppLanguage.tr(context, 'No session logs yet'),
        message: AppLanguage.tr(
          context,
          'Enable “Save session logs” in Settings → Logs to record new sessions.',
        ),
      );
    }
    return ListView(
      padding: const EdgeInsets.symmetric(horizontal: 34, vertical: 4),
      children: [
        for (final entry in _logs)
          _LogRow(
            key: ValueKey('log-${entry.file}'),
            entry: entry,
            selected: _open?.file == entry.file,
            onTap: () => _openLog(entry),
            onDelete: () => _delete(entry),
          ),
      ],
    );
  }
}

class _LogRow extends StatefulWidget {
  final SessionLogEntry entry;
  final bool selected;
  final VoidCallback onTap;
  final VoidCallback onDelete;

  const _LogRow({
    super.key,
    required this.entry,
    required this.selected,
    required this.onTap,
    required this.onDelete,
  });

  @override
  State<_LogRow> createState() => _LogRowState();
}

class _LogRowState extends State<_LogRow> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final entry = widget.entry;
    return MouseRegion(
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: 56,
          margin: const EdgeInsets.only(bottom: 2),
          padding: const EdgeInsets.symmetric(horizontal: 8),
          decoration: BoxDecoration(
            color: widget.selected
                ? XtColors.selected(context)
                : (_hover ? XtColors.hover(context) : Colors.transparent),
            borderRadius: BorderRadius.circular(XtRadius.card),
          ),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      _formatDate(entry.started),
                      style: TextStyle(
                        fontSize: 13,
                        color: XtColors.textPrimary(context),
                      ),
                    ),
                    const SizedBox(height: 2),
                    Text(
                      [
                        entry.name.isEmpty ? entry.host : entry.name,
                        if (entry.user.isNotEmpty) entry.user,
                        _formatSize(entry.size),
                      ].join('  ·  '),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(
                        fontSize: 11.5,
                        color: XtColors.textSecondary(context),
                      ),
                    ),
                  ],
                ),
              ),
              if (_hover)
                XtActionButton(
                  key: ValueKey('log-row-delete-${entry.file}'),
                  icon: LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  tooltip: AppLanguage.tr(context, 'Delete log'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: widget.onDelete,
                ),
            ],
          ),
        ),
      ),
    );
  }

  static String _formatDate(DateTime value) {
    String two(int n) => n.toString().padLeft(2, '0');
    return '${value.year}-${two(value.month)}-${two(value.day)} '
        '${two(value.hour)}:${two(value.minute)}';
  }

  static String _formatSize(int bytes) {
    if (bytes < 1024) return '$bytes B';
    if (bytes < 1024 * 1024) return '${(bytes / 1024).toStringAsFixed(1)} KB';
    return '${(bytes / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
}

class _LogViewer extends StatelessWidget {
  final SessionLogEntry entry;
  final SessionLogContent? content;
  final bool reading;
  final VoidCallback onDelete;
  final VoidCallback onClose;

  const _LogViewer({
    required this.entry,
    required this.content,
    required this.reading,
    required this.onDelete,
    required this.onClose,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      decoration: BoxDecoration(
        color: XtColors.page(context),
        border: Border(left: BorderSide(color: XtColors.hairline(context))),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            padding: const EdgeInsets.fromLTRB(16, 10, 10, 10),
            decoration: BoxDecoration(
              border: Border(
                bottom: BorderSide(color: XtColors.hairline(context)),
              ),
            ),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        entry.name.isEmpty ? entry.host : entry.name,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.typography.semiBold.copyWith(
                          fontSize: 15,
                          color: XtColors.textPrimary(context),
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        '${_LogRowState._formatDate(entry.started)}'
                        '${entry.user.isEmpty ? '' : '  ·  ${entry.user}'}'
                        '  ·  ${_LogRowState._formatSize(entry.size)}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: TextStyle(
                          fontSize: 11.5,
                          color: XtColors.textTertiary(context),
                        ),
                      ),
                    ],
                  ),
                ),
                XtActionButton(
                  key: const ValueKey('log-copy'),
                  icon: LucideIcons.copy,
                  tooltip: AppLanguage.tr(context, 'Copy transcript'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: content == null
                      ? null
                      : () => Clipboard.setData(
                          ClipboardData(text: content!.text),
                        ),
                ),
                const SizedBox(width: 4),
                XtActionButton(
                  key: const ValueKey('log-delete'),
                  icon: LucideIcons.trash2,
                  variant: XtActionVariant.danger,
                  tooltip: AppLanguage.tr(context, 'Delete log'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: onDelete,
                ),
                const SizedBox(width: 4),
                XtActionButton(
                  icon: LucideIcons.x,
                  tooltip: AppLanguage.tr(context, 'Close viewer'),
                  height: 30,
                  padding: const EdgeInsets.symmetric(horizontal: 8),
                  onPressed: onClose,
                ),
              ],
            ),
          ),
          Expanded(
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 16, 16),
              child: Container(
                decoration: BoxDecoration(
                  color: XtColors.sunken(context),
                  borderRadius: BorderRadius.circular(XtRadius.card),
                  border: Border.all(color: XtColors.hairline(context)),
                ),
                child: reading
                    ? Center(
                        child: Text(
                          AppLanguage.tr(context, 'Opening transcript…'),
                          style: TextStyle(
                            color: XtColors.textTertiary(context),
                          ),
                        ),
                      )
                    : (content?.text.isEmpty ?? true)
                    ? Center(
                        child: Padding(
                          padding: const EdgeInsets.all(24),
                          child: Text(
                            content?.closeReason == null ||
                                    content!.closeReason!.isEmpty
                                ? AppLanguage.tr(
                                    context,
                                    'This session produced no terminal output.',
                                  )
                                : AppLanguage.tr(
                                    context,
                                    'Session closed: {reason}',
                                    {'reason': content!.closeReason!},
                                  ),
                            textAlign: TextAlign.center,
                            style: TextStyle(
                              fontSize: 13,
                              color: XtColors.textTertiary(context),
                            ),
                          ),
                        ),
                      )
                    : SingleChildScrollView(
                        padding: const EdgeInsets.all(16),
                        child: Text(
                          content!.text,
                          style: TextStyle(
                            fontFamily: 'Consolas',
                            fontFamilyFallback: const ['Menlo', 'Courier New'],
                            fontSize: 12.5,
                            height: 1.5,
                            color: XtColors.textPrimary(context),
                          ),
                        ),
                      ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}
