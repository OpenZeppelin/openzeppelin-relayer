import http from "k6/http";
import { check, sleep } from "k6";

export const options = {
  vus: 2000,          // virtual users
  duration: "90s",  // test duration
};

export default function () {
  const params = {
    headers: {
      "Content-Type": "application/json",
      "Authorization": "Bearer 2cc7ec17-45ca-4498-ba86-517ef0788b8c",
    },
  };
  const res = http.get("http://localhost:8080/api/v1/relayers", params);
  check(res, { "status is 200": (r) => r.status === 200 });
  sleep(1);
}
