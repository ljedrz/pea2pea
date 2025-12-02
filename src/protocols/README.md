# Protocols

This directory contains the protocols available to nodes implementing the `Pea2Pea` trait.

## Connection Lifecycle Graph

[PNG format](https://github.com/ljedrz/pea2pea/blob/master/assets/connection_lifetime.png)

```
               +----------------+
               | New Connection |
               +-------+--------+
                       |
                   CONNECTING
                       |
                       v
            /---------------------\
            |  Handshake Enabled? |
            \----------+----------/
                       |
          +------------+-------------+
          | Yes                      | No
          v                          |
+-----------------------------+      |
| Handshake::perform_handshake|      |
+-------------+---------------+      |
              |                      |
      +-------+-------+              |
      | Failure       | Success      |
      |               |              |
      |               v              v
      |             /------------------\
      |             | Reading Enabled? |
      |             \--------+---------/
      |                      |
      |             +--------+---------+
      |             | Yes              | No
      |             v                  |
      | +------------------------------------------------+
      | | Spawn task for decoding Reading::Messages      |
      | | Spawn task for processing Reading::Messages    |
      | | (Reads will start when CONNECTED)              |
      | +------------------------+-----------------------+
      |                          |
      |                          v
      |                 /------------------\
      |                 | Writing Enabled? |
      |                 \--------+---------/
      |                          |
      |                 +--------+--------+
      |                 | Yes             | No
      |                 v                 |
      |       +-------------------------+ |
      |       | Spawn task for encoding | |
      |       | Writing::Messages       | |
      |       +------------+------------+ |
      |                    |              |
      |                    v              v
      |          +--------------------------+
      |          |  The Connection is Ready |
      |          +-----------+--------------+
      |                      |
      |                  CONNECTED
      |                      |
      |                      v
      |             /--------------------\
      |             | OnConnect Enabled? |
      |             \----------+---------/
      |                        |
      |             +----------+----------+
      |             | Yes                 | No
      |             v                     |
      |    +-----------------------+      |
      |    | OnConnect::on_connect |      |
      |    +-----------+-----------+      |
      |                |                  |
      |                v                  v
      |             [ ... ] (Active Connection)
      |                      |
      |                  disconnect
      |                      |
      |                      v
      |          /-----------------------\
      |          | OnDisconnect Enabled? |
      |          \-----------+-----------/
      |                      |
      |            +---------+----------+
      |            | Yes                | No
      |            v                    |
      | +-----------------------------+ |
      | | OnDisconnect::on_disconnect | |
      | +------------+----------------+ |
      |              |                  |
      v              v                  v
+-------------------------------------------+
|           Connection Terminated           |
+-------------------------------------------+
```
