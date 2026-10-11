import httpx
import requests

SHOP = "http://shop:8000"

svc = httpx.Client(base_url=SHOP)
sess = requests.Session()


def sync():
    svc.get("/api/users")
    svc.post("/api/orders", json={})
    sess.get("http://shop:8000/health")
    requests.get("http://shop:8000/api/users")


class Notifier:
    def __init__(self):
        self.session = requests.Session()

    def nudge(self, url):
        self.session.get(url)
