import 'dart:io';

void main() {
  print('Verifying packages are loaded from Hatch cache...\n');
  
  // Read package_config.json to show where packages come from
  var packageConfig = File('.dart_tool/package_config.json').readAsStringSync();
  
  // Check if it contains the Hatch cache path
  if (packageConfig.contains('.hatch/cache/packages')) {
    print('✅ SUCCESS: Packages are loaded from Hatch cache!');
    print('\nExample package locations:');
    
    // Extract and show a few package paths
    var lines = packageConfig.split('\n');
    var count = 0;
    for (var line in lines) {
      if (line.contains('"rootUri":') && line.contains('.hatch/cache') && count < 3) {
        var path = line.split('"rootUri":')[1].split('"')[1];
        print('  • $path');
        count++;
      }
    }
  } else {
    print('❌ ERROR: Packages are NOT from Hatch cache');
  }
}
