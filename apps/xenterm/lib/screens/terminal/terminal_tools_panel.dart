import 'package:flutter/services.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../l10n/app_language.dart';
import '../../theme/app_fonts.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';

/// Connection state of the pane's live PTY, distilled for the tools panel.
///
/// The panel never sees the full screen snapshot: it only needs this state
/// plus the throttled counters/resources published by the terminal view, so a
/// 20 fps screen refresh does not rebuild the 300px panel.
enum TerminalSessionStatus { connecting, connected, disconnected }

/// The desktop terminal's right-hand tools: live search over the visible
/// buffer, session status, font size, and real actions on the running PTY.
/// No placeholder controls — everything here acts on the live session.
class TerminalToolsPanel extends StatelessWidget {
  final AppFonts fonts;
  final TerminalSessionStatus status;
  final String search;
  final FocusNode? searchFocus;
  final int matchCount;

  /// Last remote resource sample (already throttled by the terminal view).
  final String? resourceState;
  final Map<String, dynamic>? resource;
  final ValueChanged<String> onSearch;
  final VoidCallback onClear;
  final VoidCallback onCopy;
  final VoidCallback onReconnect;
  final List<QuickCommand> snippets;
  final ValueChanged<QuickCommand>? onRunSnippet;

  const TerminalToolsPanel({
    super.key,
    required this.fonts,
    required this.status,
    required this.search,
    this.searchFocus,
    required this.matchCount,
    this.resourceState,
    this.resource,
    required this.onSearch,
    required this.onClear,
    required this.onCopy,
    required this.onReconnect,
    this.snippets = const [],
    this.onRunSnippet,
  });

  @override
  Widget build(BuildContext context) {
    final closed = status == TerminalSessionStatus.disconnected;
    final statusText = switch (status) {
      TerminalSessionStatus.connecting => AppLanguage.tr(context, 'Connecting…'),
      TerminalSessionStatus.connected => AppLanguage.tr(context, 'Connected'),
      TerminalSessionStatus.disconnected => AppLanguage.tr(
        context,
        'Disconnected',
      ),
    };
    return Container(
      decoration: BoxDecoration(
        color: XtColors.surface(context),
        border: Border(left: BorderSide(color: XtColors.hairline(context))),
      ),
      padding: const EdgeInsets.fromLTRB(14, 14, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          XtField(
            key: const ValueKey('terminal-search'),
            focusNode: searchFocus,
            onChanged: onSearch,
            placeholder: AppLanguage.tr(context, 'Search terminal'),
            leading: Icon(
              LucideIcons.search,
              size: 14,
              color: XtColors.textTertiary(context),
            ),
          ),
          const SizedBox(height: 6),
          Text(
            search.trim().isEmpty
                // Shortcuts are not translated: they name literal keys.
                ? 'Ctrl+Shift+F'
                : matchCount == 1
                ? AppLanguage.tr(context, '1 match in view')
                : AppLanguage.tr(
                    context,
                    '{count} matches in view',
                    {'count': matchCount},
                  ),
            style: TextStyle(
              fontSize: 11,
              color: XtColors.textTertiary(context),
            ),
          ),
          const SizedBox(height: 18),
          _PanelLabel(AppLanguage.tr(context, 'SESSION')),
          const SizedBox(height: 8),
          Row(
            children: [
              Container(
                width: 8,
                height: 8,
                decoration: BoxDecoration(
                  color: closed
                      ? XtColors.danger(context)
                      : XtColors.accent(context),
                  shape: BoxShape.circle,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  statusText,
                  style: TextStyle(
                    fontSize: 13,
                    color: closed
                        ? XtColors.dangerText(context)
                        : XtColors.textPrimary(context),
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: 18),
          _PanelLabel(AppLanguage.tr(context, 'SYSTEM')),
          const SizedBox(height: 8),
          _ResourceSection(state: resourceState, sample: resource),
          const SizedBox(height: 18),
          _PanelLabel(AppLanguage.tr(context, 'SNIPPETS')),
          const SizedBox(height: 8),
          if (snippets.isEmpty)
            Text(
              AppLanguage.tr(context, 'No saved snippets'),
              style: TextStyle(
                fontSize: 12,
                color: XtColors.textTertiary(context),
              ),
            )
          else
            for (final snippet in snippets.take(6))
              XtHoverRow(
                key: ValueKey('terminal-snippet-${snippet.index}'),
                onTap: () => onRunSnippet?.call(snippet),
                margin: const EdgeInsets.only(bottom: 6),
                padding: const EdgeInsets.symmetric(
                  horizontal: 8,
                  vertical: 6,
                ),
                radius: XtRadius.menu,
                child: Row(
                  children: [
                    Icon(
                      LucideIcons.terminal,
                      size: 13,
                      color: XtColors.textSecondary(context),
                    ),
                    const SizedBox(width: 7),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            snippet.name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontSize: 12,
                              color: XtColors.textPrimary(context),
                            ),
                          ),
                          const SizedBox(height: 1),
                          Text(
                            snippet.command,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: TextStyle(
                              fontFamily: 'Consolas',
                              fontFamilyFallback: const [
                                'Menlo',
                                'Courier New',
                              ],
                              fontSize: 11,
                              color: XtColors.textTertiary(context),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
          const SizedBox(height: 18),
          _PanelLabel(AppLanguage.tr(context, 'TERMINAL')),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: Text(
                  AppLanguage.tr(context, 'Font size'),
                  style: TextStyle(
                    fontSize: 13,
                    color: XtColors.textPrimary(context),
                  ),
                ),
              ),
              XtIconButton(
                key: const ValueKey('terminal-size-down'),
                icon: LucideIcons.minus,
                size: 30,
                iconSize: 13,
                onPressed: () =>
                    fonts.setTerminalSize(fonts.terminalSize - 1),
              ),
              SizedBox(
                width: 34,
                child: Text(
                  '${fonts.terminalSize.round()}',
                  textAlign: TextAlign.center,
                  style: TextStyle(
                    fontSize: 13,
                    color: XtColors.textPrimary(context),
                  ),
                ),
              ),
              XtIconButton(
                key: const ValueKey('terminal-size-up'),
                icon: LucideIcons.plus,
                size: 30,
                iconSize: 13,
                onPressed: () =>
                    fonts.setTerminalSize(fonts.terminalSize + 1),
              ),
            ],
          ),
          const SizedBox(height: 18),
          Row(
            children: [
              Expanded(
                child: XtActionButton(
                  icon: LucideIcons.copy,
                  label: AppLanguage.tr(context, 'Copy'),
                  variant: XtActionVariant.secondary,
                  height: 34,
                  expand: true,
                  onPressed: onCopy,
                ),
              ),
              const SizedBox(width: 8),
              Expanded(
                child: XtActionButton(
                  icon: LucideIcons.eraser,
                  label: AppLanguage.tr(context, 'Clear'),
                  variant: XtActionVariant.secondary,
                  height: 34,
                  expand: true,
                  onPressed: onClear,
                ),
              ),
            ],
          ),
          const SizedBox(height: 8),
          XtActionButton(
            icon: LucideIcons.refreshCw,
            label: AppLanguage.tr(context, 'Reconnect'),
            variant: XtActionVariant.secondary,
            height: 34,
            expand: true,
            onPressed: onReconnect,
          ),
        ],
      ),
    );
  }
}

class _PanelLabel extends StatelessWidget {
  final String text;
  const _PanelLabel(this.text);

  @override
  Widget build(BuildContext context) {
    return Text(
      text,
      style: TextStyle(
        fontSize: 11,
        fontWeight: FontWeight.w600,
        letterSpacing: 0.6,
        color: XtColors.textTertiary(context),
      ),
    );
  }
}

/// Live remote resource sample from the SSH monitor channel.
///
/// Every state is stated rather than hidden: `waiting` while the auxiliary
/// channel starts, `unsupported`/`unavailable` for hosts whose probe cannot
/// answer, `stale` when a previously-good sample stopped updating.
class _ResourceSection extends StatelessWidget {
  final String? state;
  final Map<String, dynamic>? sample;

  const _ResourceSection({required this.state, required this.sample});

  @override
  Widget build(BuildContext context) {
    final message = switch (state) {
      'unsupported' => AppLanguage.tr(context, 'Not available on this host'),
      'unavailable' => AppLanguage.tr(context, 'No /proc stats from this host'),
      'paused' => AppLanguage.tr(context, 'Sampling paused'),
      'stale' => AppLanguage.tr(context, 'Last sample is stale'),
      'waiting' => AppLanguage.tr(context, 'Measuring…'),
      _ when sample == null => AppLanguage.tr(context, 'Measuring…'),
      _ => null,
    };
    if (message != null) {
      // Warnings stay quiet (secondary text); unsupported states are tertiary
      // because they are informational rather than an error.
      final warning = state == 'stale' || state == 'paused';
      return Text(
        message,
        key: const ValueKey('terminal-resource-empty'),
        style: TextStyle(
          fontSize: 12,
          color: warning
              ? XtColors.textSecondary(context)
              : XtColors.textTertiary(context),
        ),
      );
    }
    final data = sample!;
    final cpu = (data['cpu_percent'] as num?)?.toDouble() ?? 0;
    final cpuSampled = data['cpu_sampled'] == true;
    final memUsed = ((data['mem_used_kib'] as num?)?.toDouble() ?? 0) / 1024;
    final memTotal = ((data['mem_total_kib'] as num?)?.toDouble() ?? 0) / 1024;
    final memFrac = memTotal <= 0 ? 0.0 : (memUsed / memTotal).clamp(0.0, 1.0);
    final disks = (data['disks'] as List?) ?? const [];
    final net = (data['net'] as List?) ?? const [];
    final gpus = (data['gpus'] as List?) ?? const [];
    final user = (data['user'] as String?) ?? '';

    var rx = 0.0;
    var tx = 0.0;
    for (final row in net) {
      if (row is Map) {
        rx += (row['rx'] as num?)?.toDouble() ?? 0;
        tx += (row['tx'] as num?)?.toDouble() ?? 0;
      }
    }

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (user.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 8),
            child: Text(
              user,
              style: TextStyle(
                fontSize: 12,
                color: XtColors.textTertiary(context),
              ),
            ),
          ),
        _ResourceMeter(
          key: const ValueKey('terminal-resource-cpu'),
          label: AppLanguage.tr(context, 'CPU'),
          value: cpuSampled ? '${(cpu * 100).clamp(0, 100).round()}%' : '—',
          fraction: cpuSampled ? cpu.clamp(0.0, 1.0) : 0,
        ),
        const SizedBox(height: 8),
        _ResourceMeter(
          key: const ValueKey('terminal-resource-memory'),
          label: AppLanguage.tr(context, 'Memory'),
          value: memTotal > 0
              ? '${_formatMib(memUsed)} / ${_formatMib(memTotal)}'
              : '—',
          fraction: memFrac,
        ),
        if (disks.isNotEmpty) ...[
          const SizedBox(height: 8),
          Builder(
            builder: (context) {
              final disk = disks.first as Map;
              final total = (disk['total'] as num?)?.toDouble() ?? 0;
              final available = (disk['available'] as num?)?.toDouble() ?? 0;
              final frac = total <= 0
                  ? 0.0
                  : ((total - available) / total).clamp(0.0, 1.0);
              return _ResourceMeter(
                key: const ValueKey('terminal-resource-disk'),
                label:
                    (disk['path'] as String?) ??
                    AppLanguage.tr(context, 'Disk'),
                value: '${(frac * 100).round()}%',
                fraction: frac,
              );
            },
          ),
        ],
        if (net.isNotEmpty) ...[
          const SizedBox(height: 8),
          _ResourceMeter(
            key: const ValueKey('terminal-resource-net'),
            label: AppLanguage.tr(context, 'Network'),
            value: '↓ ${_formatRate(rx)}  ↑ ${_formatRate(tx)}',
            fraction: 0,
          ),
        ],
        if (gpus.isNotEmpty) ...[
          const SizedBox(height: 8),
          for (final gpu in gpus.take(2))
            Padding(
              key: const ValueKey('terminal-resource-gpu'),
              padding: const EdgeInsets.only(bottom: 6),
              child: _ResourceMeter(
                label: ((gpu as Map)['name'] as String?) ?? 'GPU',
                value:
                    '${(((gpu['util_percent'] as num?)?.toDouble() ?? 0)).round()}%'
                    '${gpu['temp_c'] != null ? ' · ${gpu['temp_c']}°C' : ''}',
                fraction:
                    (((gpu['util_percent'] as num?)?.toDouble() ?? 0) / 100)
                        .clamp(0.0, 1.0),
              ),
            ),
        ] else if (data['gpu_unsupported'] == true) ...[
          const SizedBox(height: 8),
          Text(
            AppLanguage.tr(context, 'GPU: unavailable'),
            style: TextStyle(
              fontSize: 11,
              color: XtColors.textTertiary(context),
            ),
          ),
        ],
      ],
    );
  }
}

/// Binary/IEC units stay literal: they are units, not prose.
String _formatMib(double mib) => mib >= 1024
    ? '${(mib / 1024).toStringAsFixed(1)} GiB'
    : '${mib.round()} MiB';

String _formatRate(double bytes) => bytes >= 1024 * 1024
    ? '${(bytes / 1024 / 1024).toStringAsFixed(1)} MiB/s'
    : bytes >= 1024
    ? '${(bytes / 1024).toStringAsFixed(1)} KiB/s'
    : '${bytes.round()} B/s';

class _ResourceMeter extends StatelessWidget {
  final String label;
  final String value;
  final double fraction;

  const _ResourceMeter({
    super.key,
    required this.label,
    required this.value,
    required this.fraction,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            Expanded(
              child: Text(
                label,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(
                  fontSize: 12,
                  color: XtColors.textSecondary(context),
                ),
              ),
            ),
            Text(
              value,
              style: TextStyle(
                fontSize: 12,
                color: XtColors.textPrimary(context),
              ),
            ),
          ],
        ),
        const SizedBox(height: 4),
        ClipRRect(
          borderRadius: BorderRadius.circular(2),
          child: LinearProgressIndicator(
            value: fraction.clamp(0.0, 1.0),
            minHeight: 4,
            backgroundColor: XtColors.hairline(context),
            color: XtColors.accent(context),
          ),
        ),
      ],
    );
  }
}

/// Copies the visible terminal text to the clipboard (no scrollback yet).
Future<void> copyScreenText(Map<String, dynamic>? screen) async {
  final rows = (screen?['rows'] as List?) ?? const [];
  final text = rows
      .map(
        (row) => ((row as Map<String, dynamic>)['text'] as String? ?? '')
            .replaceFirst(RegExp(r' +$'), ''),
      )
      .join('\n')
      .replaceFirst(RegExp(r'\n+$'), '\n');
  await Clipboard.setData(ClipboardData(text: text));
}
