import secrets

from locust import FastHttpUser, task


class TornadoUser(FastHttpUser):
    queue_id = None
    shard = None

    def on_start(self):
        self.queue_id = secrets.token_hex(16)
        self.shard = 9800
        self.client.post(
            "/api/v1/events/internal",
            {"queue_id": self.queue_id},
            headers={"x-tornado-shard": str(self.shard)},
        )

    @task
    def hello_world(self):
        self.client.get(
            f"/json/events?queue_id={self.queue_id}&dont_block=true",
            name="/json/events?queue_id=...&dont_block=true",
        )
