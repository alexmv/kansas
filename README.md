# kansas

`kansas` is a custom reverse proxy which is used internally as part of
[Zulip](https://github.com/zulip/zulip). Its purpose is to sit
between the nginx load-balancer and the [Tornado-based real-time event
system][tornado], and forward connections from clients to their
appropriate back-end shards. It is not necessary unless there is more
than one Tornado shard.

[tornado]: https://zulip.readthedocs.io/en/latest/overview/architecture-overview.html#django-and-tornado

## Constraints

- The database is authoritative for where a realm's queues are.
- Durability
  - In the event of a failure of the service, it should not result in dropping all
    queues on the floor.
  - Memcached is not durable, so is not enough to satisfy the previous criteria.
  - The default codepath should not require a database access.
- We may want a concept of a "default" assignment which uses consistent hashing
  to not have to make an explicit choice about which backend to use.
  - However, not all realms are the same size; it is useful to be able to pin
    some large realms to be on different shards from each other.
  - This also allows us to slowly shift load off of a shard before shutting it
    down, in order to not shock-load the shards by suddenly moving a large
    number of queues.
- Given that we want to be able to move queues between shards
  - We want to generally have all queues for a realm on the same shard, since that
    makes for a better common experience in case of shard outage, and minimizes
    messages on the rabbitmq (or eventually PostgreSQL) bus.
  - While sharding is configured at the realm level, it _happens_ at the queue
    level. Which means that moving a realm requires locking and serializing
    all queues -- or relaxing the constraint that realms are only ever on one
    shard.
  - Enabling realms to move piecewise, and not atomically, also allows us to
    spread the load on the backend tornado instances for doing the queue
    serialization, at the cost of an extra message in the rabbitmq bus per shard
    that the realm is on. This is not a high cost.
  - The durable storage in the database knows nothing about queue-ids, however,
    which means that moves have to happen at the granularity of users, at very
    least.

# Queue moves

## Queue moves with page reloads

- Commit change to the database
- 400 with `BAD_EVENT_QUEUE_ID` anything which comes in which would go to the
  old shard
  - Can either do this invalidation at the kansas level, or by enqueuing
    something to the old Tornado shard which tells it to drop the relevant
    queues.

## Transparent queue moves

- Create queues in new shard
- Commit change to database
- Ensure nothing is writing to old shards using stale database data (..somehow?)
- Ensure old shard has consumed all of the data
- Pause requests from clients
- Serialize queues in old shard
- Send to new shard
- Merge serialized data with anything which has been sent to the queue already
- Resume requests, but to the new shard

# Restarts of Kansas without connection drops

- Open a UNIX socket to transfer state
- Fork and exec the new binary
- Old process writes the listen socket (port 9799) into the UNIX socket
- New process reads the socket out and calls `listen()` but not `accept()`
- New process sends a signal (e.g. `USR1`) to the old one that it has started and is ready
- Old process stops closes the listen socket
- Old process serializes the state of its internal state to the UNIX socket
- New process reads the state
- New process starts the `accept()` calls on the listen socket
- Old process finishes any in-flight requests
- Old process exits gracefully

Supervisor won't like this, because it wants to be the parent PID of what it
monitors. We can work around that by making the first process supervisor runs
be the one in change of mediating all of the above actions, so it never exits
(unless its one child exits) and is the parent of the actual `kansas` programs
which do request handling.

Additionally `supervisorctl stop kansas` will always do a full `stop` / `start`.
[As of Supervisor 3.2.0][signal], you can `supervisorctl signal SIGUSR1` or
what-not, however.

[signal]: https://github.com/Supervisor/supervisor/blob/master/CHANGES.rst#320-2015-11-30

# Restarts of Tornado without connection drops

- On connection close mid-request from tornado, or new request with dead tornado
- 502 anything that's not a GET (e.g. DELETE's are not safe to manufacture)
- Add socket to waiting queue for that shard
- Wait until earlier of 50s or backend comes back
- Respond with empty response, or maybe some synthetic "waiting" if hit 50s
- Add rate-limit before popping next one off, to spread request load

## TODO

- Load-test
  - Make a pretend Tornado service in Rust which sleeps 1min and returns a noop
- Garbage-collection of queues after 10 minutes, to match Tornado
- Persist the listen socket, using https://github.com/tormol/uds
- Persist the internal state
  - send in-memory store using serde over a socket
  - write into a local sqlite database
  - store in redis
  - just talk to postgres directly
- Paper over Tornado restarts
- Request tracing?
