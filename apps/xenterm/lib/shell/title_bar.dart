/// The custom titlebar: a 50px tab strip where the WHOLE bar is a drag
/// surface (window_manager's DragToMoveArea), with the tabs, pills and window
/// controls as interactive children on top of it.
///
/// Two sibling layers, not a wrapper: the drag/double-click surface sits
/// behind the interactive row so a pill click never waits for the double-tap
/// timeout.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:window_manager/window_manager.dart';

import '../data/hosts.dart';
import '../l10n/app_language.dart';
import '../theme/app_theme.dart';
import '../widgets/primitives.dart';

class TitleBar extends StatelessWidget {
  /// Open terminal sessions, rendered as tabs in the ONE title bar — the shell
  /// never stacks a second tab strip inside the terminal.
  final List<Host> openHosts;
  final String? activeHostId;
  final ValueChanged<String>? onSelectHost;
  final ValueChanged<String>? onCloseHost;
  final VoidCallback? onNewTab;
  final bool showSftp;
  final VoidCallback? onToggleSftp;
  final VoidCallback? onToggleSidebar;

  const TitleBar({
    super.key,
    this.openHosts = const [],
    this.activeHostId,
    this.onSelectHost,
    this.onCloseHost,
    this.onNewTab,
    this.showSftp = false,
    this.onToggleSftp,
    this.onToggleSidebar,
  });

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 50,
      child: Stack(
        children: [
          Positioned.fill(
            child: DragToMoveArea(
              child: GestureDetector(
                behavior: HitTestBehavior.opaque,
                onDoubleTap: () async {
                  if (await windowManager.isMaximized()) {
                    await windowManager.unmaximize();
                  } else {
                    await windowManager.maximize();
                  }
                },
                child: ColoredBox(
                  color: xtTitleBarColor(Theme.of(context).brightness),
                ),
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.only(left: 10),
            child: Row(
              children: [
                XtIconButton(
                  key: const ValueKey('toggle-sidebar'),
                  icon: LucideIcons.list,
                  iconSize: 14,
                  size: 30,
                  onPressed: onToggleSidebar,
                ),
                const SizedBox(width: 5),
                _SftpPill(
                  active: showSftp,
                  onTap: openHosts.isEmpty ? null : onToggleSftp,
                ),
                if (openHosts.isNotEmpty) ...[
                  const SizedBox(width: 8),
                  Container(
                    width: 1,
                    height: 22,
                    color: XtColors.hairline(context),
                  ),
                ],
                for (final host in openHosts) ...[
                  const SizedBox(width: 5),
                  _SessionTab(
                    host: host,
                    active: host.id == activeHostId,
                    onSelect: () => onSelectHost?.call(host.id),
                    onClose: () => onCloseHost?.call(host.id),
                  ),
                ],
                const SizedBox(width: 5),
                XtIconButton(
                  icon: LucideIcons.plus,
                  iconSize: 14,
                  size: 30,
                  onPressed: onNewTab ?? () {},
                ),
                const Spacer(),
                const _WindowControls(),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// One open session tab: 36px tall, radius 10, active tab filled above the
/// titlebar. The close affordance only intercepts the pointer on the active
/// tab so an inactive tab still selects on click.
class _SessionTab extends StatefulWidget {
  final Host host;
  final bool active;
  final VoidCallback onSelect;
  final VoidCallback onClose;

  const _SessionTab({
    required this.host,
    required this.active,
    required this.onSelect,
    required this.onClose,
  });

  @override
  State<_SessionTab> createState() => _SessionTabState();
}

class _SessionTabState extends State<_SessionTab> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final fg = widget.active
        ? XtColors.textPrimary(context)
        : (_hovering ? XtColors.textPrimary(context) : XtColors.textSecondary(context));
    final background = widget.active
        ? XtColors.hoverStrong(context)
        : (_hovering ? XtColors.hover(context) : Colors.transparent);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        key: ValueKey('tab-${widget.host.id}'),
        behavior: HitTestBehavior.opaque,
        onTap: widget.onSelect,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: 36,
          constraints: const BoxConstraints(maxWidth: 240),
          padding: const EdgeInsets.only(left: 10, right: 10),
          decoration: BoxDecoration(
            color: background,
            borderRadius: BorderRadius.circular(XtRadius.field),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (widget.active)
                GestureDetector(
                  key: ValueKey('tab-close-${widget.host.id}'),
                  behavior: HitTestBehavior.opaque,
                  onTap: widget.onClose,
                  child: Icon(LucideIcons.x, size: 14, color: fg),
                )
              else
                Icon(LucideIcons.x, size: 14, color: fg),
              const SizedBox(width: 8),
              Flexible(
                child: Text(
                  widget.host.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.typography.normal.copyWith(
                    fontSize: 14,
                    color: fg,
                  ),
                ),
              ),
              const SizedBox(width: 8),
              Icon(LucideIcons.squareTerminal, size: 15, color: fg),
            ],
          ),
        ),
      ),
    );
  }
}

/// The SFTP tab: a filled chip with the folder icon and label; it toggles the
/// active terminal's SFTP view and is disabled while no session is open.
class _SftpPill extends StatefulWidget {
  final bool active;
  final VoidCallback? onTap;

  const _SftpPill({required this.active, this.onTap});

  @override
  State<_SftpPill> createState() => _SftpPillState();
}

class _SftpPillState extends State<_SftpPill> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final enabled = widget.onTap != null;
    final fg = !enabled
        ? XtColors.textDisabled(context)
        : (widget.active || _hovering
              ? XtColors.textPrimary(context)
              : XtColors.textSecondary(context));
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        key: const ValueKey('sftp-tab'),
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: 30,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: widget.active
                ? XtColors.selected(context)
                : XtColors.sunken(context),
            borderRadius: BorderRadius.circular(XtRadius.field),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(LucideIcons.folderOpen, size: 15, color: fg),
              const SizedBox(width: 8),
              Text(
                AppLanguage.tr(context, 'SFTP'),
                style: theme.typography.normal.copyWith(
                  fontSize: 14,
                  color: fg,
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// min / max / close, the Windows trio, drawn — the native frame is hidden.
class _WindowControls extends StatelessWidget {
  const _WindowControls();

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        _ControlButton(icon: LucideIcons.minus, onTap: windowManager.minimize),
        _ControlButton(
          icon: LucideIcons.square,
          onTap: () async {
            if (await windowManager.isMaximized()) {
              await windowManager.unmaximize();
            } else {
              await windowManager.maximize();
            }
          },
        ),
        _ControlButton(
          icon: LucideIcons.x,
          // Windows' own close-button red, kept as the platform convention.
          hoverColor: const Color(0xFFE81123),
          onTap: windowManager.close,
        ),
      ],
    );
  }
}

class _ControlButton extends StatefulWidget {
  final IconData icon;
  final VoidCallback onTap;
  final Color? hoverColor;

  const _ControlButton({
    required this.icon,
    required this.onTap,
    this.hoverColor,
  });

  @override
  State<_ControlButton> createState() => _ControlButtonState();
}

class _ControlButtonState extends State<_ControlButton> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        child: SizedBox(
          width: 46,
          height: 50,
          child: Icon(
            widget.icon,
            size: 14,
            color: _hovering
                ? (widget.hoverColor != null
                      ? Colors.white
                      : XtColors.textPrimary(context))
                : XtColors.textSecondary(context),
          ),
        ),
      ),
    );
  }
}
