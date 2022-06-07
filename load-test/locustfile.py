import base64
from typing import Optional

from locust import FastHttpUser, events, task  # type: ignore
from locust.exception import RescheduleTask


@events.init_command_line_parser.add_listener
def init_parser(parser):
    parser.add_argument("--email", help="Email address to auth with")
    parser.add_argument(
        "--api-key", help="API key for the email address"
    )
    parser.add_argument("--block", action="store_true")


class TornadoUser(FastHttpUser):
    queue_id: Optional[str] = None
    name: Optional[str] = None
    url: Optional[str] = None

    def on_start(self):
        opts = self.environment.parsed_options

        self.headers = {}
        if opts.email and opts.api_key:
            auth_bytes = (opts.email + ":" + opts.api_key).encode()
            self.headers["Authorization"] = "Basic " + base64.b64encode(auth_bytes).decode(),
        }

        self.client.verify = False
        data = None
        with self.client.post(
            "/api/v1/register", headers=self.headers, catch_response=True
        ) as resp:
            if resp.status_code != 200:
                resp.failure(
                    f"Bad registration status code response: {resp.status_code} -> {resp.text}"
                )
            else:
                try:
                    data = resp.json()
                    if data.get("result") != "success":
                        resp.failure(f"Unsuccessful registration: {resp.text}")
                except Exception:
                    resp.failure(f"Invalid JSON response to registration: {resp.text}")
        assert data is not None

        self.queue_id = data["queue_id"]
        self.name = "/json/events?queue_id=...&last_event_id=-1&dont_block=" + (
            "false" if opts.block else "true"
        )
        self.url = self.name.replace("...", self.queue_id)

    @task
    def get_requests(self):
        with self.client.get(
            self.url, name=self.name, headers=self.headers, catch_response=True
        ) as resp:
            if resp.status_code != 200:
                resp.failure(
                    f"Bad status code response: {resp.status_code} -> {resp.text}"
                )
            elif "success" not in resp.text:
                resp.failure(f"Bad response: {resp.text}")
            else:
                resp.success()
