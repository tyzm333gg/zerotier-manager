# Maintainer: Ваш Никнейм <ваш-email@example.com>
pkgname=zerotier-manager
pkgver=0.2.0
pkgrel=1
pkgdesc="GTK4 / Libadwaita графический интерфейс для ZeroTier"
arch=('x86_64')
url="https://zerotier.com"
license=('MIT')

# Зависимости, необходимые для работы программы:
depends=(
    'libadwaita'
    'gtk4'
    'zerotier-one'
    'polkit'
    'iproute2'
    'iputils'
)

# Зависимости, нужные только для компиляции:
makedepends=(
    'cargo'
    'rust'
)

# Исходные файлы (в данном случае локальный .desktop файл)
source=(
    "zerotier-manager.desktop"
)
# 'SKIP' означает пропуск сверки контрольной суммы для локального файла
sha256sums=(
    'SKIP'
)

build() {
    # Сборка в релизном режиме (с оптимизациями -O3)
    cargo build --release --locked
}

package() {
    install -Dm755 "$startdir/target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
    install -Dm644 "$startdir/$pkgname.desktop" "$pkgdir/usr/share/applications/$pkgname.desktop"
}
