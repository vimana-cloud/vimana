from unittest import TestCase, main

class TestStringMethods(TestCase):

    def test_upper(self):
        assert 5 == 0
        # 'localhost:50051'
        #api = Client.get_by_endpoint('api.vimana.host:61803')
        #assert api.service_names == ['helloworld.Greeter']
        #request_data = {'name': 'sinsky'}
        #say_hello_response = api.request('helloworld.Greeter', 'SayHello', request_data)
        #self.assertEqual(say_hello_response, {'message': 'Hello sinsky!'})

if __name__ == '__main__':
    main()