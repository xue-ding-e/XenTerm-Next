/// Lucide icons that post-date the icon set bundled with shadcn_flutter.
///
/// The bundled font has `fingerprint` but not the newer `fingerprint-pattern`;
/// rendering the upstream 24×24 stroke SVG keeps the rail icon identical to the
/// rest of the Lucide set (stroke width 2, round caps/joins) without forking the
/// icon font.
library;

import 'package:flutter/widgets.dart';
import 'package:flutter_svg/flutter_svg.dart';

const _fingerprintPatternSvg = '''
<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M12 10a2 2 0 0 0-2 2c0 1.02-.1 2.51-.26 4" />
  <path d="M14 13.12c0 2.38 0 6.38-1 8.88" />
  <path d="M17.29 21.02c.12-.6.43-2.3.5-3.02" />
  <path d="M2 12a10 10 0 0 1 18-6" />
  <path d="M2 16h.01" />
  <path d="M21.8 16c.2-2 .131-5.354 0-6" />
  <path d="M5 19.5C5.5 18 6 15 6 12a6 6 0 0 1 .34-2" />
  <path d="M8.65 22c.21-.66.45-1.32.57-2" />
  <path d="M9 6.8a6 6 0 0 1 9 5.2v2" />
</svg>
''';

const _clipboardClockSvg = '''
<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <path d="M16 14v2.2l1.6 1" />
  <path d="M16 4h2a2 2 0 0 1 2 2v.832" />
  <path d="M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h2" />
  <circle cx="16" cy="16" r="6" />
  <rect x="8" y="2" width="8" height="4" rx="1" />
</svg>
''';

/// Lucide's `fingerprint-pattern`, tinted with [color].
class FingerprintPatternIcon extends StatelessWidget {
  final double size;
  final Color color;

  const FingerprintPatternIcon({
    super.key,
    this.size = 16,
    required this.color,
  });

  @override
  Widget build(BuildContext context) {
    return SvgPicture.string(
      _fingerprintPatternSvg,
      width: size,
      height: size,
      colorFilter: ColorFilter.mode(color, BlendMode.srcIn),
    );
  }
}

/// Lucide's `clipboard-clock`, tinted with [color].
class ClipboardClockIcon extends StatelessWidget {
  final double size;
  final Color color;

  const ClipboardClockIcon({super.key, this.size = 16, required this.color});

  @override
  Widget build(BuildContext context) {
    return SvgPicture.string(
      _clipboardClockSvg,
      width: size,
      height: size,
      colorFilter: ColorFilter.mode(color, BlendMode.srcIn),
    );
  }
}
