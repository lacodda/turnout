FROM debian:bookworm-slim
RUN apt-get update -qq && apt-get install -y -qq openssh-server nginx >/dev/null \
 && useradd -m -s /bin/bash deploy && echo deploy:sesame | chpasswd \
 && mkdir -p /run/sshd /srv/web /srv/web.backups && chown deploy /srv/web /srv/web.backups \
 && printf '#!/bin/sh\nsleep 1\n' > /usr/local/bin/web-reload && chmod +x /usr/local/bin/web-reload \
 && printf 'server {\n  listen 80 default_server;\n  root /srv/web;\n  location = /health { return 200 "ok\n"; }\n}\n' > /etc/nginx/sites-enabled/default
CMD ["sh", "-c", "nginx && /usr/sbin/sshd -D -e"]
