/// XenTerm's single source of truth for colour, radius, spacing and motion.
///
/// The app ships one neutral palette: a near-black-to-white ramp with white as
/// the accent in dark mode and near-black as the accent in light mode. The only
/// chromatic colour left is the functional danger red, so errors and
/// destructive actions remain unmistakable.
///
/// Every screen reads its colours from [XtColors] (brightness-aware) or the
/// [XtPalette] ramp instead of spelling out hex values. Adding a one-off
/// `Color(0xFF...)` in a widget is a bug: it is how the UI drifted into several
/// slightly different greys.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

/// The neutral ramp shared by both themes. Names describe the role, not the
/// light/dark direction: `black*` are surfaces, `grey*` are text tones.
abstract final class XtPalette {
  // Dark surfaces, lightest at the top of the ladder.
  static const Color black950 = Color(0xFF0A0A0A); // terminal, titlebar
  static const Color black900 = Color(0xFF111111); // window / page bg
  static const Color black850 = Color(0xFF161616); // sunken panels
  static const Color black800 = Color(0xFF1B1B1B); // cards, menus, popovers
  static const Color black750 = Color(0xFF1F1F1F); // hairline fills
  static const Color black700 = Color(0xFF262626); // elevated tiles
  static const Color black600 = Color(0xFF2E2E2E); // inputs, buttons
  static const Color black500 = Color(0xFF3A3A3A); // strong borders, chips

  // Text tones, brightest first.
  static const Color white = Color(0xFFFFFFFF); // primary text / accent
  static const Color grey200 = Color(0xFFD4D4D4); // strong body text
  static const Color grey250 = Color(0xFF8A8A8A); // tertiary text
  static const Color grey300 = Color(0xFFA6A6A6); // secondary text
  static const Color grey400 = Color(0xFF6E6E6E); // disabled text

  // Light surfaces ("paper").
  static const Color paperBase = Color(0xFFFAFAFA);
  static const Color paperSunken = Color(0xFFF2F2F2);
  static const Color paperCard = Color(0xFFFFFFFF);
  static const Color paperText = Color(0xFF171717);
  static const Color paperSecondary = Color(0xFF5C5C5C);
  static const Color paperTertiary = Color(0xFF8A8A8A);
  static const Color paperDisabled = Color(0xFFA6A6A6);
  static const Color paperField = Color(0xFFF0F0F0);
  static const Color paperHairline = Color(0xFFE6E6E6);
}

/// Functional colours that must keep their meaning: errors, destructive
/// actions and their tints. Everything else is neutral on purpose.
abstract final class XtDanger {
  static const Color solid = Color(0xFFE5484D);
  static const Color tint = Color(0x33E5484D);
  static const Color light = Color(0xFFFF8A8D);
}

/// Semantic colours resolved against the active brightness.
///
/// Widgets call these instead of hard-coding a dark-only hex value, which is
/// what previously made the light theme half-broken: a panel painted for the
/// dark ramp stayed dark on paper.
abstract final class XtColors {
  static bool _dark(BuildContext context) =>
      Theme.of(context).brightness == Brightness.dark;

  /// Window / page background.
  static Color page(BuildContext context) =>
      _dark(context) ? XtPalette.black900 : XtPalette.paperBase;

  /// Titlebar, navigation rail and other window chrome.
  static Color chrome(BuildContext context) =>
      _dark(context) ? XtPalette.black950 : XtPalette.paperSunken;

  /// Cards, panels, menus and popovers (one step off the page).
  static Color surface(BuildContext context) =>
      _dark(context) ? XtPalette.black800 : XtPalette.paperCard;

  /// Sunken surface inside a panel (terminal background, list well).
  static Color sunken(BuildContext context) =>
      _dark(context) ? XtPalette.black850 : XtPalette.paperSunken;

  /// Text inputs, selects and filled secondary buttons.
  static Color field(BuildContext context) =>
      _dark(context) ? XtPalette.black600 : XtPalette.paperField;

  /// Transient hover wash.
  static Color hover(BuildContext context) => _dark(context)
      ? Colors.white.withValues(alpha: 0.06)
      : Colors.black.withValues(alpha: 0.05);

  /// Hover wash on top of a card or field.
  static Color hoverStrong(BuildContext context) => _dark(context)
      ? XtPalette.black700
      : Colors.black.withValues(alpha: 0.07);

  /// Selected / active chip.
  static Color selected(BuildContext context) => _dark(context)
      ? XtPalette.black500
      : Colors.black.withValues(alpha: 0.10);

  /// Hairline separators (dividers, card borders that should stay quiet).
  static Color hairline(BuildContext context) =>
      _dark(context) ? XtPalette.black700 : XtPalette.paperHairline;

  /// Visibly stronger border (inputs, focused surfaces).
  static Color border(BuildContext context) => _dark(context)
      ? Colors.white.withValues(alpha: 0.16)
      : Colors.black.withValues(alpha: 0.14);

  /// Primary text.
  static Color textPrimary(BuildContext context) =>
      _dark(context) ? XtPalette.white : XtPalette.paperText;

  /// Secondary text: labels, subtitles, hints.
  static Color textSecondary(BuildContext context) =>
      _dark(context) ? XtPalette.grey300 : XtPalette.paperSecondary;

  /// Tertiary text: timestamps, counters, footnotes.
  static Color textTertiary(BuildContext context) =>
      _dark(context) ? XtPalette.grey250 : XtPalette.paperTertiary;

  /// Disabled text and icons.
  static Color textDisabled(BuildContext context) =>
      _dark(context) ? XtPalette.grey400 : XtPalette.paperDisabled;

  /// Accent fill (primary button, selected switch track).
  static Color accent(BuildContext context) =>
      _dark(context) ? XtPalette.white : XtPalette.paperText;

  /// Content colour on top of [accent].
  static Color onAccent(BuildContext context) =>
      _dark(context) ? XtPalette.black950 : XtPalette.paperCard;

  /// Destructive / error foreground.
  static Color danger(BuildContext context) =>
      _dark(context) ? XtDanger.solid : const Color(0xFFD92D20);

  /// Destructive / error background tint.
  static Color dangerTint(BuildContext context) =>
      _dark(context) ? XtDanger.tint : const Color(0x14D92D20);

  /// Destructive / error text on [dangerTint].
  static Color dangerText(BuildContext context) =>
      _dark(context) ? XtDanger.light : const Color(0xFFB42318);

  /// Menu / popover drop shadow.
  static Color shadow(BuildContext context) =>
      _dark(context) ? const Color(0x66000000) : const Color(0x1F000000);

  /// Terminal background (always the darkest surface, in both themes).
  static Color terminal(BuildContext context) =>
      _dark(context) ? XtPalette.black950 : const Color(0xFF111111);
}

/// Titlebar / tab-strip fill: one step off the content surface in both modes.
Color xtTitleBarColor(Brightness brightness) =>
    brightness == Brightness.dark
        ? XtPalette.black950
        : XtPalette.paperSunken;

/// The hairline under the titlebar.
Color xtTitleBarBorderColor(Brightness brightness) =>
    brightness == Brightness.dark
        ? XtPalette.black700
        : Colors.black.withValues(alpha: 0.10);

ColorScheme xtDarkScheme() {
  const base = XtPalette.black900;
  const cards = XtPalette.black800;
  const forms = XtPalette.black600;
  final hover = Colors.white.withValues(alpha: 0.06);
  final border = Colors.white.withValues(alpha: 0.16);
  return ColorScheme(
    brightness: Brightness.dark,
    background: base,
    foreground: XtPalette.white,
    card: cards,
    cardForeground: XtPalette.white,
    popover: cards,
    popoverForeground: XtPalette.white,
    primary: XtPalette.white,
    primaryForeground: XtPalette.black950,
    secondary: forms,
    secondaryForeground: XtPalette.grey200,
    muted: hover,
    mutedForeground: XtPalette.grey300,
    accent: hover,
    accentForeground: XtPalette.white,
    destructive: XtDanger.solid,
    destructiveForeground: XtPalette.white,
    border: border,
    input: forms,
    ring: XtPalette.white,
    chart1: XtPalette.white,
    chart2: XtPalette.grey300,
    chart3: const Color(0xFF8C8C8C),
    chart4: const Color(0xFF666666),
    chart5: const Color(0xFF4D4D4D),
  );
}

ColorScheme xtLightScheme() {
  const text = XtPalette.paperText;
  final border = Colors.black.withValues(alpha: 0.14);
  final hover = Colors.black.withValues(alpha: 0.05);
  return ColorScheme(
    brightness: Brightness.light,
    background: XtPalette.paperBase,
    foreground: text,
    card: XtPalette.paperCard,
    cardForeground: text,
    popover: XtPalette.paperCard,
    popoverForeground: text,
    primary: text,
    primaryForeground: XtPalette.paperCard,
    secondary: XtPalette.paperField,
    secondaryForeground: text,
    muted: hover,
    mutedForeground: XtPalette.paperSecondary,
    accent: hover,
    accentForeground: text,
    destructive: const Color(0xFFD92D20),
    destructiveForeground: XtPalette.white,
    border: border,
    input: XtPalette.paperField,
    ring: text,
    chart1: text,
    chart2: const Color(0xFF555555),
    chart3: const Color(0xFF777777),
    chart4: const Color(0xFF999999),
    chart5: const Color(0xFFBBBBBB),
  );
}

/// Radius slots. The theme multiplier stays at 0.5 for library internals; app
/// surfaces call these constants so a card is the same card everywhere.
abstract final class XtRadius {
  static const double menu = 6;
  static const double card = 8;
  static const double field = 10;
  static const double cardLarge = 15;
  static const double pill = 1000;
}

/// Shared layout metrics.
abstract final class XtMetrics {
  /// Height of a toolbar / header strip.
  static const double headerHeight = 56;

  /// The search field's inner height.
  static const double searchFieldHeight = 36;

  /// HostsFiltersHeader: height 45, padding 4/12, gap 15.
  static const double toolbarHeight = 45;

  /// The hosts grid card's on-disk width (it may shrink in a narrower pane).
  static const double gridCardWidth = 430;

  /// Standard control height for inputs, selects and buttons.
  static const double controlHeight = 36;

  /// Height of a settings row.
  static const double rowHeight = 44;

  /// The nav rail's expanded width.
  static const double navRailWidth = 182;
}

/// Duration / easing slots used by every implicit animation.
abstract final class XtMotion {
  /// Micro feedback: colour fades, hover reveals.
  static const Duration micro = Duration(milliseconds: 200);

  /// Panels, sections, rail.
  static const Duration panel = Duration(milliseconds: 300);

  /// The faster half of a collapse.
  static const Duration collapse = Duration(milliseconds: 200);

  /// easeOut `cubic-bezier(0, 0, 0.2, 1)`.
  static const Curve easeOut = Cubic(0, 0, 0.2, 1);

  /// easeInOut `cubic-bezier(0.4, 0, 0.2, 1)`.
  static const Curve easeInOut = Cubic(0.4, 0, 0.2, 1);
}
