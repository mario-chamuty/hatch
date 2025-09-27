library test_plugin;

class TestPlugin {
  static const String version = '1.0.0';

  static void initialize() {
    print('Test Plugin initialized');
  }

  static String greet(String name) {
    return 'Hello, $name from Test Plugin!';
  }
}