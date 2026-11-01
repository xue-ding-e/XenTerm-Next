/// Toolbar building blocks shared by every screen: the split [icon Label | ▾]
/// button, the ghost icon+label action, the square icon toggle and the section
/// title.
///
/// Each one is a thin, API-stable wrapper over the single implementation in
/// [primitives] — the old file carried its own copy of the hover colours,
/// popup surface and radii, which is exactly how the screens drifted apart.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../theme/app_theme.dart';
import 'primitives.dart';

/// One entry of a [SplitButton] menu.
class SplitMenuEntry {
  final IconData icon;
  final String label;
  final VoidCallback onTap;
  const SplitMenuEntry({
    required this.icon,
    required this.label,
    required this.onTap,
  });
}

/// [+ Label | ▾] — two segments 2px apart; the chevron rotates while open and
/// an empty [menuItems] list hides the chevron entirely.
class SplitButton extends StatefulWidget {
  final String label;
  final IconData icon;
  final VoidCallback? onPressed;
  final List<SplitMenuEntry>? menuItems;

  const SplitButton({
    super.key,
    required this.label,
    required this.icon,
    this.onPressed,
    this.menuItems,
  });

  @override
  State<SplitButton> createState() => _SplitButtonState();
}

class _SplitButtonState extends State<SplitButton> {
  final OverlayPortalController _menu = OverlayPortalController();
  final LayerLink _link = LayerLink();

  void _toggleMenu() => setState(() {
    if (_menu.isShowing) {
      _menu.hide();
    } else {
      _menu.show();
    }
  });

  @override
  Widget build(BuildContext context) {
    const outer = Radius.circular(XtRadius.card);
    const inner = Radius.circular(2);
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        _SplitSegment(
          onTap: widget.onPressed,
          radius: const BorderRadius.only(
            topLeft: outer,
            bottomLeft: outer,
            topRight: inner,
            bottomRight: inner,
          ),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(
                widget.icon,
                size: 14,
                color: XtColors.textPrimary(context),
              ),
              const SizedBox(width: 8),
              Text(
                widget.label,
                style: Theme.of(context).typography.medium.copyWith(
                  fontSize: 14,
                  color: XtColors.textPrimary(context),
                ),
              ),
            ],
          ),
        ),
        if (widget.menuItems?.isNotEmpty ?? false) ...[
          const SizedBox(width: 2),
          CompositedTransformTarget(
            link: _link,
            child: _SplitSegment(
              onTap: _toggleMenu,
              radius: const BorderRadius.only(
                topLeft: inner,
                bottomLeft: inner,
                topRight: outer,
                bottomRight: outer,
              ),
              padding: const EdgeInsets.symmetric(horizontal: 10),
              child: AnimatedRotation(
                turns: _menu.isShowing ? 0.5 : 0,
                duration: XtMotion.micro,
                curve: XtMotion.easeOut,
                child: Icon(
                  LucideIcons.chevronDown,
                  size: 14,
                  color: XtColors.textPrimary(context),
                ),
              ),
            ),
          ),
          OverlayPortal(controller: _menu, overlayChildBuilder: _overlay),
        ],
      ],
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
          offset: const Offset(0, 8),
          showWhenUnlinked: false,
          child: Align(
            alignment: Alignment.topLeft,
            child: XtMenuPopup(
              width: 300,
              children: [
                for (final item in widget.menuItems!)
                  XtMenuEntry(
                    icon: item.icon,
                    label: item.label,
                    onTap: () {
                      setState(_menu.hide);
                      item.onTap();
                    },
                  ),
              ],
            ),
          ),
        ),
      ],
    );
  }
}

/// One split-button segment: its own rounded rect, hover one step lighter.
class _SplitSegment extends StatefulWidget {
  final VoidCallback? onTap;
  final EdgeInsetsGeometry padding;
  final BorderRadius radius;
  final Widget child;

  const _SplitSegment({
    required this.onTap,
    required this.padding,
    required this.radius,
    required this.child,
  });

  @override
  State<_SplitSegment> createState() => _SplitSegmentState();
}

class _SplitSegmentState extends State<_SplitSegment> {
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
        child: AnimatedContainer(
          duration: XtMotion.micro,
          curve: XtMotion.easeOut,
          height: XtMetrics.controlHeight,
          padding: widget.padding,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: _hovering
                ? XtColors.hoverStrong(context)
                : XtColors.field(context),
            borderRadius: widget.radius,
          ),
          child: widget.child,
        ),
      ),
    );
  }
}

/// Ghost toolbar action: icon + label, hover chip.
class GhostAction extends StatelessWidget {
  final IconData icon;
  final String label;
  final VoidCallback? onPressed;

  const GhostAction({
    super.key,
    required this.icon,
    required this.label,
    this.onPressed,
  });

  @override
  Widget build(BuildContext context) {
    return XtActionButton(
      icon: icon,
      label: label,
      onPressed: onPressed,
      variant: XtActionVariant.ghost,
    );
  }
}

/// Square icon toggle (grid/tag/search/keyboard…).
class IconToggleButton extends StatelessWidget {
  final IconData icon;
  final VoidCallback? onPressed;

  const IconToggleButton({super.key, required this.icon, this.onPressed});

  @override
  Widget build(BuildContext context) {
    return XtIconButton(icon: icon, onPressed: onPressed);
  }
}

/// The bold section title over a screen's content ("Hosts", "Keys"…).
class SectionTitle extends StatelessWidget {
  final String text;

  const SectionTitle({super.key, required this.text});

  @override
  Widget build(BuildContext context) => XtSectionTitle(text);
}
