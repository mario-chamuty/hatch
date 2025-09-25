library http;

// Mock HTTP client for testing
class Client {
  Future<Response> get(Uri url) async {
    return Response('Mock response from forked HTTP client', 200);
  }
}

class Response {
  final String body;
  final int statusCode;

  Response(this.body, this.statusCode);
}