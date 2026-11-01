/// Shared UI primitives.
///
/// Every screen used to grow its own card, hover row, switch, dropdown, action
/// button or empty state. They looked almost-but-not-quite the same, and the
/// light theme broke wherever a widget had baked in a dark hex value. This file
/// is the single implementation for those patterns: screens compose these
/// widgets and read colours from [XtColors], so a change here reaches the whole
/// app at once.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../l10n/app_language.dart';
import '../theme/app_theme.dart';

/// A surface card: theme surface fill, hairline border and [XtRadius.card].
///
/// [interactive] adds the shared hover wash and a click cursor; [onTap] alone
/// is enough to opt into that behaviour.
class XtCard extends StatefulWidget {
  final Widget child;
  final EdgeInsetsGeometry padding;
  final EdgeInsetsGeometry? margin;
  final double radius;
  final bool border;
  final bool interactive;
  final Color? color;
  final VoidCallback? onTap;

  const XtCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(16),
    this.margin,
    this.radius = XtRadius.card,
    this.border = true,
    this.interactive = false,
    this.color,
    this.onTap,
  });

  @override
  State<XtCard> createState() => _XtCardState();
}

class _XtCardState extends State<XtCard> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final fill = widget.color ?? XtColors.surface(context);
    final hovered = widget.onTap != null || widget.interactive;
    final background = hovered && _hovering
        ? Color.alphaBlend(XtColors.hover(context), fill)
        : fill;
    final card = Container(
      margin: widget.margin,
      padding: widget.padding,
      decoration: BoxDecoration(
        color: background,
        borderRadius: BorderRadius.circular(widget.radius),
        border: widget.border
            ? Border.all(color: XtColors.hairline(context))
            : null,
      ),
      child: widget.child,
    );
    if (!hovered) return card;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onTap,
        child: card,
      ),
    );
  }
}

/// A bold section title above a group of content.
class XtSectionTitle extends StatelessWidget {
  final String text;
  final EdgeInsetsGeometry padding;

  const XtSectionTitle(
    this.text, {
    super.key,
    this.padding = const EdgeInsets.fromLTRB(32, 16, 0, 12),
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: padding,
      child: Text(
        text,
        style: theme.typography.bold.copyWith(
          fontSize: 18,
          color: XtColors.textPrimary(context),
        ),
      ),
    );
  }
}

/// A panel/header strip: optional leading glyph, a 16px semibold title and
/// trailing actions, on a chrome surface with a hairline bottom border.
class XtPanelHeader extends StatelessWidget {
  final String title;
  final IconData? leading;
  final List<Widget> actions;
  final double height;
  final EdgeInsetsGeometry padding;

  const XtPanelHeader({
    super.key,
    required this.title,
    this.leading,
    this.actions = const [],
    this.height = XtMetrics.headerHeight,
    this.padding = const EdgeInsets.only(left: 15, right: 10),
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      height: height,
      padding: padding,
      decoration: BoxDecoration(
        color: XtColors.surface(context),
        border: Border(bottom: BorderSide(color: XtColors.hairline(context))),
      ),
      child: Row(
        children: [
          if (leading != null) ...[
            Icon(leading, size: 16, color: XtColors.textSecondary(context)),
            const SizedBox(width: 8),
          ],
          Expanded(
            child: Text(
              title,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: theme.typography.semiBold.copyWith(
                fontSize: 16,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
          ...actions,
        ],
      ),
    );
  }
}

/// The visual weight of an [XtActionButton].
enum XtActionVariant {
  /// Accent fill: the one primary action on a surface.
  primary,

  /// Filled secondary action.
  secondary,

  /// Text/icon action that only gains a background on hover.
  ghost,

  /// Destructive text action.
  danger,
}

/// One action button for toolbars, cards and headers.
///
/// Replaces the per-screen `_HeaderButton`, `_CardAction`, `_ToolAction`,
/// `_PanelAction` and `GhostAction` copies, which differed only in shade.
class XtActionButton extends StatefulWidget {
  final IconData? icon;
  final String? label;
  final VoidCallback? onPressed;
  final XtActionVariant variant;
  final double height;
  final double iconSize;
  final EdgeInsetsGeometry? padding;
  final String? tooltip;
  final bool expand;

  const XtActionButton({
    super.key,
    this.icon,
    this.label,
    this.onPressed,
    this.variant = XtActionVariant.ghost,
    this.height = XtMetrics.controlHeight,
    this.iconSize = 15,
    this.padding,
    this.tooltip,
    this.expand = false,
  });

  @override
  State<XtActionButton> createState() => _XtActionButtonState();
}

class _XtActionButtonState extends State<XtActionButton> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final enabled = widget.onPressed != null;
    final (fill, fg) = switch (widget.variant) {
      XtActionVariant.primary => (
        enabled
            ? XtColors.accent(context)
            : Color.alphaBlend(
                XtColors.hover(context),
                XtColors.accent(context),
              ),
        XtColors.onAccent(context),
      ),
      XtActionVariant.secondary => (
        _hovering && enabled
            ? XtColors.hoverStrong(context)
            : XtColors.field(context),
        enabled
            ? XtColors.textPrimary(context)
            : XtColors.textDisabled(context),
      ),
      XtActionVariant.danger => (
        _hovering && enabled ? XtColors.dangerTint(context) : Colors.transparent,
        enabled ? XtColors.dangerText(context) : XtColors.textDisabled(context),
      ),
      XtActionVariant.ghost => (
        _hovering && enabled ? XtColors.hover(context) : Colors.transparent,
        enabled
            ? XtColors.textSecondary(context)
            : XtColors.textDisabled(context),
      ),
    };
    final radius = BorderRadius.circular(XtRadius.card);
    final content = Container(
      height: widget.height,
      padding:
          widget.padding ??
          EdgeInsets.symmetric(horizontal: widget.label == null ? 10 : 12),
      decoration: BoxDecoration(color: fill, borderRadius: radius),
      child: Row(
        mainAxisSize: widget.expand ? MainAxisSize.max : MainAxisSize.min,
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          if (widget.icon != null)
            Icon(widget.icon, size: widget.iconSize, color: fg),
          if (widget.icon != null && widget.label != null)
            const SizedBox(width: 8),
          if (widget.label != null)
            Flexible(
              child: Text(
                widget.label!,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.typography.medium.copyWith(
                  fontSize: 13.5,
                  color: fg,
                ),
              ),
            ),
        ],
      ),
    );
    Widget target = MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onPressed,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          child: content,
        ),
      ),
    );
    if (widget.tooltip != null) {
      target = Tooltip(tooltip: (_) => Text(widget.tooltip!), child: target);
    }
    return target;
  }
}

/// A square icon-only button with the shared hover chip.
class XtIconButton extends StatefulWidget {
  final IconData icon;
  final VoidCallback? onPressed;
  final String? tooltip;
  final double size;
  final double iconSize;
  final bool selected;

  const XtIconButton({
    super.key,
    required this.icon,
    this.onPressed,
    this.tooltip,
    this.size = 32,
    this.iconSize = 15,
    this.selected = false,
  });

  @override
  State<XtIconButton> createState() => _XtIconButtonState();
}

class _XtIconButtonState extends State<XtIconButton> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onPressed != null;
    final background = widget.selected
        ? XtColors.selected(context)
        : (_hovering && enabled ? XtColors.hover(context) : Colors.transparent);
    final fg = enabled
        ? (widget.selected
              ? XtColors.textPrimary(context)
              : XtColors.textSecondary(context))
        : XtColors.textDisabled(context);
    Widget target = MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.onPressed,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          width: widget.size,
          height: widget.size,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: background,
            borderRadius: BorderRadius.circular(XtRadius.card),
          ),
          child: Icon(widget.icon, size: widget.iconSize, color: fg),
        ),
      ),
    );
    if (widget.tooltip != null) {
      target = Tooltip(tooltip: (_) => Text(widget.tooltip!), child: target);
    }
    return target;
  }
}

/// The single switch: 48×28 track with a constant 20px thumb.
class XtSwitch extends StatelessWidget {
  final bool value;
  final ValueChanged<bool>? onChanged;

  const XtSwitch({super.key, required this.value, this.onChanged});

  @override
  Widget build(BuildContext context) {
    final enabled = onChanged != null;
    final track = value
        ? (enabled ? XtColors.accent(context) : XtColors.textDisabled(context))
        : XtColors.selected(context);
    final thumb = XtColors.onAccent(context);
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: enabled ? () => onChanged!(!value) : null,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          width: 48,
          height: 28,
          padding: const EdgeInsets.all(4),
          decoration: BoxDecoration(
            color: track,
            borderRadius: BorderRadius.circular(20),
          ),
          child: Align(
            alignment: value ? Alignment.centerRight : Alignment.centerLeft,
            child: Container(
              width: 20,
              height: 20,
              decoration: BoxDecoration(color: thumb, shape: BoxShape.circle),
            ),
          ),
        ),
      ),
    );
  }
}

/// A settings row: label (and optional description) on the left, switch on the
/// right.
class XtSwitchRow extends StatelessWidget {
  final String label;
  final String? description;
  final bool value;
  final ValueChanged<bool>? onChanged;

  const XtSwitchRow({
    super.key,
    required this.label,
    required this.value,
    this.onChanged,
    this.description,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: 20),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(
                  AppLanguage.tr(context, label),
                  style: theme.typography.medium.copyWith(
                    fontSize: 13,
                    color: XtColors.textPrimary(context),
                  ),
                ),
              ),
              XtSwitch(value: value, onChanged: onChanged),
            ],
          ),
          if (description != null) ...[
            const SizedBox(height: 4),
            Text(
              AppLanguage.tr(context, description!),
              style: theme.typography.normal.copyWith(
                fontSize: 13,
                color: XtColors.textTertiary(context),
              ),
            ),
          ],
        ],
      ),
    );
  }
}

/// A list/tab/rail row with the shared hover and selection washes.
class XtHoverRow extends StatefulWidget {
  final Widget child;
  final VoidCallback? onTap;
  final bool selected;
  final EdgeInsetsGeometry padding;
  final EdgeInsetsGeometry? margin;
  final double? height;
  final double radius;
  final bool enabled;

  const XtHoverRow({
    super.key,
    required this.child,
    this.onTap,
    this.selected = false,
    this.padding = const EdgeInsets.symmetric(horizontal: 10),
    this.margin,
    this.height,
    this.radius = XtRadius.card,
    this.enabled = true,
  });

  @override
  State<XtHoverRow> createState() => _XtHoverRowState();
}

class _XtHoverRowState extends State<XtHoverRow> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final background = widget.selected
        ? XtColors.selected(context)
        : (_hovering && widget.enabled && widget.onTap != null
              ? XtColors.hover(context)
              : Colors.transparent);
    return MouseRegion(
      cursor: widget.onTap == null
          ? SystemMouseCursors.basic
          : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: widget.enabled ? widget.onTap : null,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: widget.height,
          margin: widget.margin,
          padding: widget.padding,
          decoration: BoxDecoration(
            color: background,
            borderRadius: BorderRadius.circular(widget.radius),
          ),
          child: widget.child,
        ),
      ),
    );
  }
}

/// One row inside an [XtMenuPopup].
class XtMenuEntry extends StatefulWidget {
  final IconData? icon;
  final String label;
  final VoidCallback? onTap;
  final bool selected;
  final bool danger;
  final Widget? trailing;

  const XtMenuEntry({
    super.key,
    this.icon,
    required this.label,
    this.onTap,
    this.selected = false,
    this.danger = false,
    this.trailing,
  });

  @override
  State<XtMenuEntry> createState() => _XtMenuEntryState();
}

class _XtMenuEntryState extends State<XtMenuEntry> {
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final enabled = widget.onTap != null;
    final fg = widget.danger
        ? (enabled ? XtColors.dangerText(context) : XtColors.textDisabled(context))
        : (enabled
              ? XtColors.textPrimary(context)
              : XtColors.textDisabled(context));
    return MouseRegion(
      cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: GestureDetector(
        behavior: HitTestBehavior.opaque,
        onTap: enabled ? widget.onTap : null,
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: 40,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: widget.selected
                ? XtColors.selected(context)
                : (_hovering && enabled
                      ? XtColors.hover(context)
                      : Colors.transparent),
            borderRadius: BorderRadius.circular(XtRadius.menu),
          ),
          child: Row(
            children: [
              if (widget.icon != null) ...[
                Icon(widget.icon, size: 16, color: fg),
                const SizedBox(width: 10),
              ],
              Expanded(
                child: Text(
                  widget.label,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: theme.typography.normal.copyWith(
                    fontSize: 13.5,
                    color: fg,
                  ),
                ),
              ),
              if (widget.trailing != null) ...[
                const SizedBox(width: 8),
                widget.trailing!,
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// The popup surface shared by every dropdown, split-button menu and overflow
/// menu.
class XtMenuPopup extends StatelessWidget {
  final List<Widget> children;
  final double? width;
  final double minWidth;
  final double? maxHeight;
  final EdgeInsetsGeometry padding;

  const XtMenuPopup({
    super.key,
    required this.children,
    this.width,
    this.minWidth = 160,
    this.maxHeight,
    this.padding = const EdgeInsets.all(5),
  });

  @override
  Widget build(BuildContext context) {
    Widget list = Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: children,
    );
    if (maxHeight != null) {
      list = SingleChildScrollView(child: list);
    }
    return Container(
      width: width,
      constraints: BoxConstraints(
        minWidth: minWidth,
        maxHeight: maxHeight ?? double.infinity,
      ),
      padding: padding,
      decoration: BoxDecoration(
        color: XtColors.surface(context),
        borderRadius: BorderRadius.circular(XtRadius.field),
        border: Border.all(color: XtColors.hairline(context)),
        boxShadow: [
          BoxShadow(
            color: XtColors.shadow(context),
            blurRadius: 18,
            offset: const Offset(0, 8),
          ),
        ],
      ),
      child: list,
    );
  }
}

/// Anchors a popup under any target widget and handles outside-click dismissal.
///
/// This owns the OverlayPortal/LayerLink dance that five screens used to copy;
/// the target and the popup contents are the only per-screen parts.
class XtMenuAnchor extends StatefulWidget {
  final Widget Function(BuildContext context, bool open, VoidCallback toggle)
  builder;
  final List<Widget> children;

  /// Alternative to [children] for menus whose rows need to close the popup
  /// after being chosen (selects especially: leaving it open reads as a bug).
  final List<Widget> Function(BuildContext context, VoidCallback close)?
  childrenBuilder;
  final double? width;
  final double minWidth;
  final double? maxHeight;
  final Offset offset;

  const XtMenuAnchor({
    super.key,
    required this.builder,
    this.children = const [],
    this.childrenBuilder,
    this.width = 300,
    this.minWidth = 160,
    this.maxHeight,
    this.offset = const Offset(0, 8),
  });

  @override
  State<XtMenuAnchor> createState() => _XtMenuAnchorState();
}

class _XtMenuAnchorState extends State<XtMenuAnchor> {
  final OverlayPortalController _menu = OverlayPortalController();
  final LayerLink _link = LayerLink();

  void _toggle() {
    setState(() {
      if (_menu.isShowing) {
        _menu.hide();
      } else {
        _menu.show();
      }
    });
  }

  @override
  Widget build(BuildContext context) {
    return OverlayPortal(
      controller: _menu,
      overlayChildBuilder: _overlay,
      child: CompositedTransformTarget(
        link: _link,
        child: widget.builder(context, _menu.isShowing, _toggle),
      ),
    );
  }

  Widget _overlay(BuildContext context) {
    return Stack(
      children: [
        Positioned.fill(
          child: GestureDetector(
            behavior: HitTestBehavior.opaque,
            onTap: () => setState(_menu.hide),
            child: const ColoredBox(color: Colors.transparent),
          ),
        ),
        CompositedTransformFollower(
          link: _link,
          targetAnchor: Alignment.bottomLeft,
          followerAnchor: Alignment.topLeft,
          offset: widget.offset,
          showWhenUnlinked: false,
          child: Align(
            alignment: Alignment.topLeft,
            child: XtMenuPopup(
              width: widget.width,
              minWidth: widget.minWidth,
              maxHeight: widget.maxHeight,
              children: widget.childrenBuilder?.call(
                    context,
                    () => setState(_menu.hide),
                  ) ??
                  widget.children,
            ),
          ),
        ),
      ],
    );
  }
}

/// A filled select field: a 36px field-looking control with the popup list
/// anchored below it.
class XtSelectField<T> extends StatefulWidget {
  final T value;
  final Map<T, String> options;
  final ValueChanged<T> onChanged;
  final double height;
  final Widget? leading;

  const XtSelectField({
    super.key,
    required this.value,
    required this.options,
    required this.onChanged,
    this.height = XtMetrics.controlHeight,
    this.leading,
  });

  @override
  State<XtSelectField<T>> createState() => _XtSelectFieldState<T>();
}

class _XtSelectFieldState<T> extends State<XtSelectField<T>> {
  @override
  Widget build(BuildContext context) {
    final label = widget.options[widget.value] ?? '${widget.value}';
    return XtMenuAnchor(
      width: 220,
      minWidth: 180,
      childrenBuilder: (context, close) => [
        for (final entry in widget.options.entries)
          XtMenuEntry(
            label: AppLanguage.tr(context, entry.value),
            selected: entry.key == widget.value,
            trailing: entry.key == widget.value
                ? Icon(
                    LucideIcons.check,
                    size: 14,
                    color: XtColors.accent(context),
                  )
                : null,
            onTap: () {
              widget.onChanged(entry.key);
              close();
            },
          ),
      ],
      builder: (context, open, toggle) => GestureDetector(
        onTap: toggle,
        child: Container(
          height: widget.height,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: XtColors.field(context),
            borderRadius: BorderRadius.circular(XtRadius.field),
          ),
          child: Row(
            children: [
              if (widget.leading != null) ...[
                widget.leading!,
                const SizedBox(width: 6),
              ],
              Expanded(
                child: Text(
                  AppLanguage.tr(context, label),
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontSize: 13.5,
                    color: XtColors.textPrimary(context),
                  ),
                ),
              ),
              Icon(
                LucideIcons.chevronDown,
                size: 14,
                color: XtColors.textTertiary(context),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// A settings row whose value is a text + chevron dropdown.
class XtSelectRow<T> extends StatelessWidget {
  final String label;
  final T value;
  final Map<T, String> options;
  final ValueChanged<T> onChanged;

  const XtSelectRow({
    super.key,
    required this.label,
    required this.value,
    required this.options,
    required this.onChanged,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.only(top: 20),
      child: Row(
        children: [
          Expanded(
            child: Text(
              AppLanguage.tr(context, label),
              style: theme.typography.medium.copyWith(
                fontSize: 13,
                color: XtColors.textPrimary(context),
              ),
            ),
          ),
          XtMenuAnchor(
            width: 200,
            childrenBuilder: (context, close) => [
              for (final entry in options.entries)
                XtMenuEntry(
                  label: AppLanguage.tr(context, entry.value),
                  selected: entry.key == value,
                  onTap: () {
                    onChanged(entry.key);
                    close();
                  },
                ),
            ],
            builder: (context, open, toggle) => GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: toggle,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Text(
                    AppLanguage.tr(context, options[value] ?? '$value'),
                    style: theme.typography.medium.copyWith(
                      fontSize: 13,
                      color: XtColors.textPrimary(context),
                    ),
                  ),
                  const SizedBox(width: 5),
                  Icon(
                    LucideIcons.chevronDown,
                    size: 12,
                    color: XtColors.textTertiary(context),
                  ),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// An empty/error placeholder: glyph, title, optional message and action.
class XtEmptyState extends StatelessWidget {
  final IconData icon;
  final String title;
  final String? message;
  final Widget? action;
  final bool danger;

  const XtEmptyState({
    super.key,
    required this.icon,
    required this.title,
    this.message,
    this.action,
    this.danger = false,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final fg = danger
        ? XtColors.dangerText(context)
        : XtColors.textTertiary(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 32, color: fg),
            const SizedBox(height: 12),
            Text(
              title,
              textAlign: TextAlign.center,
              style: theme.typography.medium.copyWith(
                fontSize: 15,
                color: danger
                    ? XtColors.dangerText(context)
                    : XtColors.textPrimary(context),
              ),
            ),
            if (message != null) ...[
              const SizedBox(height: 6),
              Text(
                message!,
                textAlign: TextAlign.center,
                style: theme.typography.normal.copyWith(
                  fontSize: 13,
                  color: XtColors.textSecondary(context),
                ),
              ),
            ],
            if (action != null) ...[const SizedBox(height: 16), action!],
          ],
        ),
      ),
    );
  }
}

/// A small pill label (status, protocol, count).
enum XtBadgeTone { neutral, accent, danger }

class XtBadge extends StatelessWidget {
  final String label;
  final XtBadgeTone tone;

  const XtBadge({
    super.key,
    required this.label,
    this.tone = XtBadgeTone.neutral,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final (fill, fg) = switch (tone) {
      XtBadgeTone.neutral => (
        XtColors.selected(context),
        XtColors.textSecondary(context),
      ),
      XtBadgeTone.accent => (
        XtColors.accent(context),
        XtColors.onAccent(context),
      ),
      XtBadgeTone.danger => (
        XtColors.dangerTint(context),
        XtColors.dangerText(context),
      ),
    };
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
      decoration: BoxDecoration(
        color: fill,
        borderRadius: BorderRadius.circular(XtRadius.menu),
      ),
      child: Text(
        label,
        style: theme.typography.medium.copyWith(fontSize: 11, color: fg),
      ),
    );
  }
}

/// A numeric stepper with the shared square buttons.
class XtStepper extends StatelessWidget {
  final String value;
  final VoidCallback? onDecrement;
  final VoidCallback? onIncrement;

  const XtStepper({
    super.key,
    required this.value,
    this.onDecrement,
    this.onIncrement,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        XtIconButton(
          icon: LucideIcons.minus,
          onPressed: onDecrement,
          size: 26,
          iconSize: 13,
        ),
        Container(
          width: 44,
          alignment: Alignment.center,
          child: Text(
            value,
            style: theme.typography.normal.copyWith(
              fontSize: 13,
              color: XtColors.textPrimary(context),
            ),
          ),
        ),
        XtIconButton(
          icon: LucideIcons.plus,
          onPressed: onIncrement,
          size: 26,
          iconSize: 13,
        ),
      ],
    );
  }
}

/// A one-pixel horizontal rule in the theme's hairline colour.
class XtDivider extends StatelessWidget {
  final EdgeInsetsGeometry? margin;
  final double height;

  const XtDivider({super.key, this.margin, this.height = 1});

  @override
  Widget build(BuildContext context) {
    return Container(
      height: height,
      color: XtColors.hairline(context),
    );
  }
}

/// A keyboard-shortcut chip.
class XtKeyChip extends StatelessWidget {
  final String label;

  const XtKeyChip({super.key, required this.label});

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      decoration: BoxDecoration(
        color: XtColors.selected(context),
        borderRadius: BorderRadius.circular(XtRadius.menu),
      ),
      child: Text(
        label,
        style: theme.typography.medium.copyWith(
          fontSize: 12,
          color: XtColors.textPrimary(context),
        ),
      ),
    );
  }
}

/// A label above a control ("Host", "Port", …).
class XtFieldLabel extends StatelessWidget {
  final String text;
  final EdgeInsetsGeometry padding;

  const XtFieldLabel(
    this.text, {
    super.key,
    this.padding = const EdgeInsets.only(bottom: 6),
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: padding,
      child: Text(
        text,
        style: theme.typography.medium.copyWith(
          fontSize: 12,
          color: XtColors.textSecondary(context),
        ),
      ),
    );
  }
}
