FROM frolvlad/alpine-glibc:latest
RUN apk add --no-cache curl bash sed busybox jq
WORKDIR /root
RUN ls && mkdir /app && cd /app && curl -sL https://ghproxy.badking.pp.ua/https://raw.githubusercontent.com/IonRh/TGBot_RSS/main/TGBot_RSS.sh | bash
COPY Docker/main.sh /app/main.sh
RUN chmod +x /app/main.sh
ENV TZ=Asia/Shanghai
# 校准时钟
RUN busybox ntpd -q -p ntp.aliyun.com
ENTRYPOINT ["/app/main.sh"]
