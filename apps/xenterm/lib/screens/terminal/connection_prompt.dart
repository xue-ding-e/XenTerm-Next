/// The blocking connect-time prompt (host key / credentials / MFA) shared by
/// the terminal and the SFTP panel. The answer goes back through the bridge to
/// the paused auth flow.
library;

import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../l10n/app_language.dart';
import '../../theme/app_field.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';

class ConnectionPromptBanner extends StatefulWidget {
  final Map<String, dynamic> prompt;
  final void Function(
    bool accept, {
    String? username,
    String? password,
    String? text,
  })
  onAnswer;

  const ConnectionPromptBanner({
    super.key,
    required this.prompt,
    required this.onAnswer,
  });

  @override
  State<ConnectionPromptBanner> createState() => ConnectionPromptBannerState();
}

class ConnectionPromptBannerState extends State<ConnectionPromptBanner> {
  late final TextEditingController _user = TextEditingController(
    text: '${widget.prompt['user'] ?? ''}',
  );
  final TextEditingController _password = TextEditingController();
  final TextEditingController _text = TextEditingController();

  @override
  void dispose() {
    _user.dispose();
    _password.dispose();
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final kind = widget.prompt['kind'] as String?;
    final fields = <Widget>[];
    if (kind == 'host_key') {
      fields.add(
        Text(
          widget.prompt['changed'] == true
              ? AppLanguage.tr(
                  context,
                  'Host key CHANGED for {host}:{port} — possible MITM.',
                  {
                    'host': '${widget.prompt['host']}',
                    'port': '${widget.prompt['port']}',
                  },
                )
              : AppLanguage.tr(context, 'Unknown host key for {host}:{port}', {
                  'host': '${widget.prompt['host']}',
                  'port': '${widget.prompt['port']}',
                }),
          style: theme.typography.medium.copyWith(
            fontSize: 13,
            color: XtColors.textPrimary(context),
          ),
        ),
      );
      fields.add(
        Padding(
          padding: const EdgeInsets.only(top: 6),
          child: Text(
            '${widget.prompt['key_type']}  ${widget.prompt['fingerprint']}',
            style: theme.typography.normal.copyWith(
              fontSize: 12,
              color: XtColors.textSecondary(context),
            ),
          ),
        ),
      );
    } else if (kind == 'credential') {
      final needUser = widget.prompt['need_user'] == true;
      if (needUser) {
        fields.add(_promptField(_user, AppLanguage.tr(context, 'Username')));
      }
      if (widget.prompt['need_password'] == true) {
        fields.add(
          _promptField(
            _password,
            AppLanguage.tr(context, 'Password'),
            obscure: true,
          ),
        );
      }
    } else {
      fields.add(
        Text(
          '${widget.prompt['prompt'] ?? AppLanguage.tr(context, 'Verification code')}',
          style: theme.typography.medium.copyWith(
            fontSize: 13,
            color: XtColors.textPrimary(context),
          ),
        ),
      );
      fields.add(
        _promptField(
          _text,
          AppLanguage.tr(context, 'Code'),
          obscure: widget.prompt['echo'] == false,
        ),
      );
    }

    return XtCard(
      padding: const EdgeInsets.all(12),
      radius: XtRadius.field,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ...fields,
          const SizedBox(height: 10),
          Row(
            mainAxisAlignment: MainAxisAlignment.end,
            children: [
              XtActionButton(
                label: AppLanguage.tr(context, 'Cancel'),
                onPressed: () => widget.onAnswer(false),
              ),
              const SizedBox(width: 8),
              XtActionButton(
                label: AppLanguage.tr(
                  context,
                  kind == 'host_key' ? 'Trust' : 'Continue',
                ),
                variant: XtActionVariant.primary,
                onPressed: () => widget.onAnswer(
                  true,
                  username: _user.text,
                  password: _password.text,
                  text: _text.text,
                ),
              ),
            ],
          ),
        ],
      ),
    );
  }

  Widget _promptField(
    TextEditingController controller,
    String hint, {
    bool obscure = false,
  }) {
    return Padding(
      padding: const EdgeInsets.only(top: 8),
      child: XtField(
        controller: controller,
        obscure: obscure,
        placeholder: hint,
      ),
    );
  }
}
