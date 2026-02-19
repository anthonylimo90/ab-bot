#!/bin/sh
set -eu

# Required secrets (provide strong values in production).
: "${REDIS_APP_PASSWORD:=app_secret_change_me}"
: "${REDIS_DYNAMIC_TUNER_PASSWORD:=dynamic_tuner_secret_change_me}"
: "${REDIS_DYNAMIC_SUBSCRIBER_PASSWORD:=dynamic_subscriber_secret_change_me}"

cat > /usr/local/etc/redis/users.acl <<EOF
user default off
user app on >${REDIS_APP_PASSWORD} ~arb:runtime:stats:latest &arb:* &copy:signals &orderbook:updates +@connection +@pubsub +get +set
user dynamic_tuner on >${REDIS_DYNAMIC_TUNER_PASSWORD} ~arb:runtime:stats:latest &dynamic:config:update +@connection +publish +get
user dynamic_subscriber on >${REDIS_DYNAMIC_SUBSCRIBER_PASSWORD} &dynamic:config:update +@connection +subscribe +psubscribe +unsubscribe +punsubscribe
EOF

exec redis-server \
  --appendonly yes \
  --aclfile /usr/local/etc/redis/users.acl \
  --protected-mode yes
