FROM httpd:2.4.68@sha256:979c38c2228d28c2edfd45c6e27dcee1c7b4a101a5526721ae8ece454e89e99e

COPY httpd.conf /usr/local/apache2/conf/httpd.conf

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get upgrade -y \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /srv/webdav \
    && /usr/local/apache2/bin/htpasswd -bc /usr/local/apache2/conf/webdav.htpasswd test test \
    && chown -R www-data:www-data /srv/webdav