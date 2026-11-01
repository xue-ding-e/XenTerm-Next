import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/shell/window_activity.dart';

void main() {
  testWidgets('resize pauses polling until the window settles', (
    tester,
  ) async {
    final activity = WindowActivity();
    addTearDown(activity.dispose);

    expect(activity.isIdle, isTrue);
    activity.resize();
    expect(activity.isIdle, isFalse);

    await tester.pump(WindowActivity.settleDelay ~/ 2);
    expect(activity.isIdle, isFalse);

    await tester.pump(WindowActivity.settleDelay);
    expect(activity.isIdle, isTrue);
  });

  testWidgets('minimize stays paused until the window is restored', (
    tester,
  ) async {
    final activity = WindowActivity();
    addTearDown(activity.dispose);

    activity.minimize();
    expect(activity.isIdle, isFalse);
    // A resize event must not wake a minimized window.
    activity.resize();
    await tester.pump(WindowActivity.settleDelay * 3);
    expect(activity.isIdle, isFalse);

    activity.restore();
    await tester.pump(WindowActivity.settleDelay);
    expect(activity.isIdle, isTrue);
  });

  testWidgets('wake cancels the settle timer', (tester) async {
    final activity = WindowActivity();
    addTearDown(activity.dispose);

    activity.resize();
    expect(activity.isIdle, isFalse);
    activity.wake();
    expect(activity.isIdle, isTrue);
    await tester.pump(WindowActivity.settleDelay * 2);
    expect(activity.isIdle, isTrue);
  });
}
