import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:noctorium_installer/main.dart';
import 'package:noctorium_installer/release.dart';

/// The screen, with a release made up for it rather than GitHub's: what is offered, and what is said, for
/// a release with Noctorium Stats in it and for one from before it.
void main() {
  Release release(String tag, List<String> names) => Release(
        tag: tag,
        assets: [for (final name in names) Asset(name: name, url: 'https://example.test/$name', size: 1)],
      );

  final withStats = release('v0.13.0', [
    'Noctorium-0.13.0.apk',
    'Noctorium-Installer-android.apk',
    'Noctorium-Stats-0.13.0.apk',
    'SHA256SUMS.txt',
  ]);
  final beforeStats = release('v0.12.2', [
    'Noctorium-0.12.2.apk',
    'Noctorium-Installer-android.apk',
    'SHA256SUMS.txt',
  ]);

  FilledButton button(WidgetTester tester, String label) =>
      tester.widget<FilledButton>(find.ancestor(of: find.text(label), matching: find.byType(FilledButton)));

  testWidgets('both are offered when the release carries both', (tester) async {
    await tester.pumpWidget(InstallerApp(lookUp: () async => withStats));
    await tester.pumpAndSettle();
    expect(button(tester, 'Install Noctorium').onPressed, isNotNull);
    expect(button(tester, 'Install Noctorium Stats').onPressed, isNotNull);
    expect(find.textContaining('your listening, in figures'), findsOneWidget);
  });

  testWidgets('Stats is not offered of a release from before it, and says why', (tester) async {
    await tester.pumpWidget(InstallerApp(lookUp: () async => beforeStats));
    await tester.pumpAndSettle();
    expect(button(tester, 'Install Noctorium').onPressed, isNotNull, reason: 'Noctorium is there to install');
    expect(button(tester, 'Install Noctorium Stats').onPressed, isNull);
    expect(find.text('Noctorium Stats is not in v0.12.2 yet.'), findsOneWidget);
  });

  testWidgets('a release it could not ask about leaves both to find out for themselves', (tester) async {
    await tester.pumpWidget(InstallerApp(lookUp: () async => throw const SocketException('offline')));
    await tester.pumpAndSettle();
    expect(button(tester, 'Install Noctorium').onPressed, isNotNull);
    expect(button(tester, 'Install Noctorium Stats').onPressed, isNotNull);
  });

  testWidgets('pressed for a release without it, Stats says so plainly and is not a failure', (tester) async {
    // The look ahead found Stats; by the time the button is pressed, the latest release is one without.
    final answers = [withStats, beforeStats];
    await tester.pumpWidget(InstallerApp(lookUp: () async => answers.removeAt(0)));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Install Noctorium Stats'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Noctorium Stats is not in v0.12.2 yet, so there is nothing to install'), findsOneWidget);
    final message = tester.widget<Text>(find.textContaining('Noctorium Stats is not in v0.12.2 yet, so'));
    final scheme = Theme.of(tester.element(find.byType(Scaffold))).colorScheme;
    expect(message.style?.color, isNot(scheme.error), reason: 'said, not complained of');
    expect(button(tester, 'Install Noctorium').onPressed, isNotNull);
    expect(button(tester, 'Install Noctorium Stats').onPressed, isNull, reason: 'and not offered again');
  });

  // Pictures of the screen, for looking at rather than testing: NOCTORIUM_RENDER_TO names a folder, and
  // Roboto is taken from the Flutter SDK's own copy so that the words are words rather than boxes.
  final renderTo = Platform.environment['NOCTORIUM_RENDER_TO'];
  testWidgets('the screen drawn', (tester) async {
    await tester.runAsync(() async {
      final fonts = '${Platform.environment['FLUTTER_ROOT']}/bin/cache/artifacts/material_fonts';
      final loader = FontLoader('Roboto');
      for (final weight in ['regular', 'medium', 'bold']) {
        final file = File('$fonts/roboto-$weight.ttf');
        if (file.existsSync()) loader.addFont(Future.value(ByteData.sublistView(file.readAsBytesSync())));
      }
      await loader.load();
    });
    tester.view.physicalSize = const Size(1080, 2160);
    tester.view.devicePixelRatio = 2.75;
    addTearDown(tester.view.reset);
    for (final (name, latest) in [('with-stats', withStats), ('before-stats', beforeStats)]) {
      final key = GlobalKey();
      await tester.pumpWidget(RepaintBoundary(key: key, child: InstallerApp(lookUp: () async => latest)));
      await tester.pumpAndSettle();
      await tester.runAsync(() async {
        final boundary = key.currentContext!.findRenderObject()! as RenderRepaintBoundary;
        final image = await boundary.toImage(pixelRatio: 1);
        final bytes = await image.toByteData(format: ui.ImageByteFormat.png);
        File('$renderTo/phone-$name.png').writeAsBytesSync(bytes!.buffer.asUint8List());
      });
    }
  }, skip: renderTo == null);
}
