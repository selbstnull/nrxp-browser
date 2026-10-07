# Тестовый «сервер» для проверки `nrb deploy`: Ubuntu 24.04 с systemd и sshd. Только для локальных тестов.
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
 && apt-get install -y --no-install-recommends systemd systemd-sysv openssh-server sudo iproute2 procps ca-certificates \
 && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash deploy \
 && echo 'deploy ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/deploy && chmod 440 /etc/sudoers.d/deploy \
 && install -d -m 700 -o deploy -g deploy /home/deploy/.ssh \
 && systemctl enable ssh
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
