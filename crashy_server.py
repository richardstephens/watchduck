
import http.server
import threading

import socketserver
import time
import sys

PORT = 8000

Handler = http.server.SimpleHTTPRequestHandler
def spawn_server():
  with socketserver.TCPServer(("", PORT), Handler) as httpd:
    print("serving at port", PORT)
    httpd.serve_forever()
x = threading.Thread(target=spawn_server, args=(), daemon=True)
x.start()

time.sleep(15)

print("crashy crash")
sys.exit(1)
