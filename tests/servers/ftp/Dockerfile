FROM debian:13-slim@sha256:d7e12182ce18b85b93007c1dedf31f2d29e01ccf3182cc4017c709b6259bc132

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends curl vsftpd \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /var/run/vsftpd/empty \
    && chmod 0555 /var/run/vsftpd/empty \
    && useradd --create-home --home-dir /srv/ftp --shell /bin/bash bifrost-dev \
    && echo "bifrost-dev:bifrost-dev-secret" | chpasswd \
    && chown -R bifrost-dev:bifrost-dev /srv/ftp

COPY vsftpd.conf /etc/vsftpd.conf

EXPOSE 21 21100-21109

CMD ["/usr/sbin/vsftpd", "/etc/vsftpd.conf"]