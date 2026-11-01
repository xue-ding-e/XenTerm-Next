/// The one text field used by every screen.
///
/// Filled with the theme's field colour, radius [XtRadius.field], 36px tall by
/// default, no heavy outline and a quiet placeholder. Wrapping the stock widget
/// once is what keeps search, host details, snippets, forwarding, serial and
/// connection prompts from each drifting into a slightly different shade,
/// height or radius.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import 'app_theme.dart';

class XtField extends StatelessWidget {
  final String placeholder;
  final String? initialValue;
  final TextEditingController? controller;
  final FocusNode? focusNode;
  final ValueChanged<String>? onChanged;
  final ValueChanged<String>? onSubmitted;
  final bool obscure;
  final bool autofocus;
  final bool enabled;
  final double fontSize;
  final double height;
  final Widget? leading;
  final Widget? trailing;
  final TextInputType? keyboardType;
  final int? maxLines;
  final int? minLines;
  final TextAlign? textAlign;
  final EdgeInsetsGeometry? padding;

  const XtField({
    super.key,
    required this.placeholder,
    this.initialValue,
    this.controller,
    this.focusNode,
    this.onChanged,
    this.onSubmitted,
    this.obscure = false,
    this.autofocus = false,
    this.enabled = true,
    this.fontSize = 13.5,
    this.height = XtMetrics.controlHeight,
    this.leading,
    this.trailing,
    this.keyboardType,
    this.maxLines = 1,
    this.minLines,
    this.textAlign,
    this.padding,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final multiline = (maxLines ?? 1) > 1;
    final radius = BorderRadius.circular(XtRadius.field);
    final fill = theme.colorScheme.secondary;
    final field = TextField(
      controller: controller,
      initialValue: initialValue,
      focusNode: focusNode,
      placeholder: Text(
        placeholder,
        style: TextStyle(fontSize: fontSize, color: XtColors.textTertiary(context)),
      ),
      onChanged: onChanged,
      onSubmitted: onSubmitted,
      obscureText: obscure,
      enabled: enabled,
      autofocus: autofocus,
      keyboardType: keyboardType,
      maxLines: maxLines,
      minLines: minLines,
      textAlign: textAlign ?? TextAlign.start,
      padding:
          padding ??
          EdgeInsets.symmetric(horizontal: 10, vertical: multiline ? 9 : 8),
      border: Border.all(color: Colors.transparent),
      borderRadius: radius,
      decoration: BoxDecoration(color: fill, borderRadius: radius),
      style: theme.typography.normal.copyWith(
        fontSize: fontSize,
        color: enabled
            ? XtColors.textPrimary(context)
            : XtColors.textDisabled(context),
      ),
    );
    if (leading == null && trailing == null && !multiline) {
      return SizedBox(height: height, child: field);
    }
    return Container(
      constraints: BoxConstraints(minHeight: height),
      padding: EdgeInsets.only(
        left: leading == null ? 10 : 9,
        right: trailing == null ? 10 : 9,
      ),
      decoration: BoxDecoration(color: fill, borderRadius: radius),
      child: Row(
        children: [
          if (leading != null) ...[leading!, const SizedBox(width: 6)],
          Expanded(child: field),
          if (trailing != null) ...[const SizedBox(width: 6), trailing!],
        ],
      ),
    );
  }
}
