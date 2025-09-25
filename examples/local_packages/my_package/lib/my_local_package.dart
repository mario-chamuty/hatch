library my_local_package;

class LocalPackageDemo {
  final String message;

  LocalPackageDemo(this.message);

  void greet() {
    print('Hello from local package: $message');
  }
}

// Example utility function
String formatMessage(String input) {
  return '[$input] - Processed by local package';
}