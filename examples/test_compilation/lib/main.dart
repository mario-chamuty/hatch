import 'package:http/http.dart' as http;

void main() async {
  print('Testing HTTP package from Hatch cache...');
  
  // This will use the http package from the Hatch cache
  var response = await http.get(Uri.parse('https://jsonplaceholder.typicode.com/posts/1'));
  
  print('Status: ${response.statusCode}');
  print('Body length: ${response.body.length}');
}
