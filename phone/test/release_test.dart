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
