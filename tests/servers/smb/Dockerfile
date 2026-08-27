FROM debian:13-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends samba smbclient \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --shell /usr/sbin/nologin bifrost-dev \
    && printf "bifrost-dev-secret\nbifrost-dev-secret\n" | smbpasswd -a -s bifrost-dev \
    && mkdir -p /srv/samba/share \
    && chown -R bifrost-dev:bifrost-dev /srv/samba/share

COPY smb.conf /etc/samba/smb.conf

EXPOSE 445

CMD ["/usr/sbin/smbd", "--foreground", "--no-process-group", "--debug-stdout"]