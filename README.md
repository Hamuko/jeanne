# jeanne

qBittorrent ratio ruler

## Description

jeanne is a program to set varying share limits in qBittorrent based on a set of rules. Rules are evaluated from top to bottom and the first matching one is applied. If none of the rules matches, global limits are used instead. All conditions in a rule are AND.

## Configuration

jeanne is configured with a Yaml file that contains connection details for the server and a list of rules. For example:

```yaml
server:
  address: https://qbittorrent.server.home.arpa/
  username: momo
  password: dandadandadanda

rules:
  # Set torrents with category "Alien" to seed to 20.0 ratio / 90 days
  # if they've already been seeding for at least seven days.
  - category: Alien
    seedingTime: ">10080"  # Prefix can be '>', '>=', '<', '<='
    limits:
      ratio: 20.0
      minutes: 129600
  # Set torrents with category "Ghost" and no tags to seed to 100.0 ratio.
  - category: Ghost
    tags: []  # Exact match
    limits:
      ratio: 100.0
```

`server.username` and `server.password` are optional if your qBittorrent server does not require authentication.
