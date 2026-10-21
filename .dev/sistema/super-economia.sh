#!/bin/sh
# Instala a Super economia do Modo de energia: o ajudante em /usr/local/bin,
# uma regra do polkit que deixa só o seu usuário rodá-lo sem senha, e um
# serviço que liga de novo no boot se ela estava ligada.
# Uso: sudo .dev/sistema/super-economia.sh  (ou pkexec, que pede a senha numa janela)
#      sudo .dev/sistema/super-economia.sh remover
set -e
# Quem pediu: pelo sudo ou pelo pkexec.
if [ -n "${PKEXEC_UID:-}" ]; then user=$(id -nu "$PKEXEC_UID"); else user=${SUDO_USER:-$USER}; fi
helper=/usr/local/bin/super-economia
rule=/etc/polkit-1/rules.d/49-super-economia.rules
unit=/etc/systemd/system/super-economia.service

if [ "${1:-}" = remover ]; then
    [ -x "$helper" ] && "$helper" off
    systemctl disable super-economia.service 2>/dev/null || true
    rm -f "$helper" "$rule" "$unit"
    rm -rf /var/lib/super-economia
    systemctl daemon-reload
    echo "ok: Super economia removida."
    exit 0
fi

install -Dm0755 "$(dirname "$0")/super-economia" "$helper"

cat > "$rule" <<RULE
// O applet Central liga e desliga a Super economia sem pedir senha.
polkit.addRule(function(action, subject) {
    if (action.id == "org.freedesktop.policykit.exec" &&
        action.lookup("program") == "$helper" &&
        subject.user == "$user" && subject.local && subject.active) {
        return polkit.Result.YES;
    }
});
RULE

cat > "$unit" <<UNIT
[Unit]
Description=Super economia de energia, se estava ligada
After=power-profiles-daemon.service systemd-modules-load.service

[Service]
Type=oneshot
ExecStart=$helper boot

[Install]
WantedBy=multi-user.target
UNIT
systemctl daemon-reload
systemctl enable super-economia.service
echo "ok: a Super economia aparece no Modo de energia do Central, para $user."
