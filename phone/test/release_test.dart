import 'package:flutter_test/flutter_test.dart';
import 'package:noctorium_installer/release.dart';

/// The half of the installer that is a function of what GitHub said, which is the half worth testing
/// without a phone in the room.
void main() {
  const releaseJson = '''
  {
    "tag_name": "v0.4.0",
    "assets": [
      {"name": "Noctorium-0.4.0-windows-x64-setup.exe", "browser_download_url": "https://example.test/setup.exe", "size": 340636160},
      {"name": "Noctorium-0.4.0.apk", "browser_download_url": "https://example.test/x.apk", "size": 17600515},
      {"name": "noctorium_0.4.0_amd64.deb", "browser_download_url": "https://example.test/x.deb", "size": 251486386},
      {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/SHA256SUMS.txt", "size": 473}
    ]
  }
  ''';

  test('it reads the tag and every file', () {
    final release = Release.fromJson(releaseJson);
    expect(release.tag, 'v0.4.0');
    expect(release.assets.length, 4);
  });

  test('the phone takes the APK and nothing else', () {
    final release = Release.fromJson(releaseJson);
    expect(release.apk?.name, 'Noctorium-0.4.0.apk');
    expect(release.apk?.size, 17600515);
    expect(release.checksums?.name, 'SHA256SUMS.txt');
  });

  test('the installer never picks itself', () {
    // It is attached to the same release as the application, so it has to skip its own APK.
    final release = Release.fromJson('''
    {
      "tag_name": "v1.0.0",
      "assets": [
        {"name": "Noctorium-Installer-android.apk", "browser_download_url": "u", "size": 1},
        {"name": "Noctorium-1.0.0.apk", "browser_download_url": "u", "size": 2}
      ]
    }
    ''');
    expect(release.apk?.name, 'Noctorium-1.0.0.apk');
  });

  // A release from 0.13 on: Noctorium's APK, Noctorium Stats' and the installer's own, listed in an order
  // that puts Stats first -- which GitHub does not do today, sorting by name, and need never promise.
  const withStats = '''
  {
    "tag_name": "v0.13.0",
    "assets": [
      {"name": "Noctorium-Stats-0.13.0.apk", "browser_download_url": "https://example.test/stats.apk", "size": 21000000},
      {"name": "Noctorium-Installer-android.apk", "browser_download_url": "https://example.test/installer.apk", "size": 9000000},
      {"name": "noctorium-stats-0.13.0-windows-x64.zip", "browser_download_url": "https://example.test/stats.zip", "size": 6400000},
      {"name": "Noctorium-0.13.0.apk", "browser_download_url": "https://example.test/noctorium.apk", "size": 17600515},
      {"name": "noctorium-cli-0.13.0-windows-x64.zip", "browser_download_url": "https://example.test/cli.zip", "size": 60000000},
      {"name": "SHA256SUMS.txt", "browser_download_url": "https://example.test/SHA256SUMS.txt", "size": 2400}
    ]
  }
  ''';

  test('Noctorium and Noctorium Stats each take their own APK, whatever order they are listed in', () {
    final release = Release.fromJson(withStats);
    expect(release.apk?.name, 'Noctorium-0.13.0.apk');
    expect(release.statsApk?.name, 'Noctorium-Stats-0.13.0.apk');
    expect(release.statsApk?.size, 21000000);

    // And the other way round, as GitHub lists them today.
    final sorted = Release(tag: release.tag, assets: release.assets.reversed.toList());
    expect(sorted.apk?.name, 'Noctorium-0.13.0.apk');
    expect(sorted.statsApk?.name, 'Noctorium-Stats-0.13.0.apk');
  });

  test('a release from before Noctorium Stats has Noctorium and no Stats', () {
    // 0.12.2 as published, less the desktop's files.
    final release = Release.fromJson('''
    {
      "tag_name": "v0.12.2",
      "assets": [
        {"name": "Noctorium-0.12.2.apk", "browser_download_url": "u", "size": 17600515},
        {"name": "Noctorium-Installer-android.apk", "browser_download_url": "u", "size": 9000000},
        {"name": "noctorium-cli-0.12.2-linux-x64.tar.gz", "browser_download_url": "u", "size": 1},
        {"name": "SHA256SUMS.txt", "browser_download_url": "u", "size": 2078}
      ]
    }
    ''');
    expect(release.apk?.name, 'Noctorium-0.12.2.apk');
    expect(release.statsApk, isNull);
  });

  test('nothing but the shape of a name answers for an APK', () {
    Release named(List<String> names) =>
        Release(tag: 'v1', assets: [for (final name in names) Asset(name: name, url: 'u', size: 1)]);
    expect(named(['Noctorium-Stats-1.0.0.apk']).apk, isNull, reason: 'Stats is never Noctorium');
    expect(named(['Noctorium-1.0.0.apk']).statsApk, isNull, reason: 'nor Noctorium Stats');
    expect(named(['Noctorium-Installer-android.apk']).statsApk, isNull);
    expect(named(['Noctorium-Stats-1.0.0.apk.sha256']).statsApk, isNull);
    expect(named(['noctorium-stats-1.0.0-macos-arm64.zip']).statsApk, isNull);
    expect(named(['noctorium-stats-1.0.0.apk']).statsApk?.name, 'noctorium-stats-1.0.0.apk');
  });

  test('a release published for the desktop only says so rather than installing an exe', () {
    final release = Release.fromJson('{"tag_name":"v9","assets":[{"name":"x.exe","browser_download_url":"u","size":1}]}');
    expect(release.apk, isNull);
    expect(release.checksums, isNull);
  });

  test('what a private repository answers is not mistaken for a release', () {
    // This is exactly what GitHub sends for a repository the caller cannot see.
    expect(() => Release.fromJson('{"message":"Not Found"}'), throwsFormatException);
    expect(() => Release.fromJson('not json at all'), throwsA(isA<FormatException>()));
  });

  test('the checksum is found by name and by nothing else', () {
    const listing = '1b6e64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e  Noctorium-0.4.0.apk\n'
        '342b82f3ccbeb41e4b2890b0b1df0dae85389f481b4405c3bfb9ed0b5cef8432  Noctorium-0.4.0-windows-x64.msi\n';
    expect(publishedChecksum(listing, 'Noctorium-0.4.0.apk'), '1b6e64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e');
    expect(publishedChecksum(listing, 'Noctorium-0.4.0.exe'), isNull);
  });

  test('a listing that is not one yields nothing rather than a wrong answer', () {
    expect(publishedChecksum('', 'x'), isNull);
    expect(publishedChecksum('garbage  x', 'x'), isNull);
    expect(
      publishedChecksum('zzzz64b90cdcf2633a69eb5cbca74e1c66b60270fcd57772d0ab205183653d3e  x', 'x'),
      isNull,
      reason: 'not every 64-character string is hexadecimal',
    );
  });
}
