/// What GitHub says the latest release is, and which of its files belongs on a phone.
///
/// Kept apart from the screen so it can be tested without one: everything here is a function of text
/// that came back from GitHub, which is exactly the part worth having tests for.
library;

import 'dart:convert';

/// One file attached to a release.
class Asset {
  const Asset({required this.name, required this.url, required this.size});

  final String name;
  final String url;
  final int size;
}

/// A release, reduced to what an installer needs.
class Release {
  const Release({required this.tag, required this.assets});

  final String tag;
  final List<Asset> assets;

  /// Reads the release JSON GitHub's API answers with.
  ///
  /// Throws [FormatException] for anything that is not a release, which includes the `{"message": "Not
  /// Found"}` a private repository answers with.
  static Release fromJson(String body) {
    final dynamic root = jsonDecode(body);
    if (root is! Map<String, dynamic>) {
      throw const FormatException('GitHub did not answer with a release');
    }
    final tag = root['tag_name'];
    final assets = root['assets'];
    if (tag is! String || assets is! List) {
      throw const FormatException('GitHub did not answer with a release');
    }
    return Release(
      tag: tag,
      assets: assets.whereType<Map<String, dynamic>>().map((asset) {
        return Asset(
          name: asset['name'] as String? ?? '',
          url: asset['browser_download_url'] as String? ?? '',
          size: asset['size'] as int? ?? 0,
        );
      }).where((asset) => asset.name.isNotEmpty && asset.url.isNotEmpty).toList(),
    );
  }

  /// Noctorium's APK, or null when the release carries none.
  ///
  /// This application is published to the same release as the one it installs, so its own APK is
  /// skipped. Without that, the installer would earnestly offer to install itself.
  Asset? get apk {
    for (final asset in assets) {
      final name = asset.name.toLowerCase();
      if (name.endsWith('.apk') && !name.contains('installer')) return asset;
    }
    return null;
  }

  Asset? get checksums {
    for (final asset in assets) {
      if (asset.name == 'SHA256SUMS.txt') return asset;
    }
    return null;
  }
}

/// The checksum published for one file, read out of a `sha256sum` listing.
///
/// The format is what coreutils writes: a hash, two spaces, a name. A line that is not that is somebody
/// else's business and is skipped rather than failing the lot.
String? publishedChecksum(String listing, String name) {
  for (final line in const LineSplitter().convert(listing)) {
    final separator = line.indexOf('  ');
    if (separator < 0) continue;
    final hash = line.substring(0, separator).trim();
    final file = line.substring(separator + 2).trim();
    if (file != name) continue;
    if (hash.length != 64 || !RegExp(r'^[0-9a-fA-F]+$').hasMatch(hash)) continue;
    return hash.toLowerCase();
  }
  return null;
}
