#include "blureffect.h"

#include <QGuiApplication>
#include <QQuickWindow>
#include <QtMath>
#include <qpa/qplatformnativeinterface.h>

#include <wayland-client.h>

namespace
{

wl_compositor *waylandCompositor()
{
    auto *native = QGuiApplication::platformNativeInterface();
    if (!native) {
        return nullptr;
    }
    return static_cast<wl_compositor *>(native->nativeResourceForIntegration("compositor"));
}

wl_surface *waylandSurface(QQuickWindow *window)
{
    auto *native = QGuiApplication::platformNativeInterface();
    if (!native || !window) {
        return nullptr;
    }
    return static_cast<wl_surface *>(native->nativeResourceForWindow("surface", window));
}

} // namespace

BlurEffect::BlurEffect()
    : QWaylandClientExtensionTemplate<BlurEffect>(1)
{
    initialize();
}

BlurEffect::~BlurEffect()
{
    for (auto *effect : std::as_const(m_effects)) {
        ext_background_effect_surface_v1_destroy(effect);
    }
}

struct ext_background_effect_surface_v1 *BlurEffect::effectForWindow(QQuickWindow *window)
{
    if (auto it = m_effects.constFind(window); it != m_effects.cend()) {
        return *it;
    }

    wl_surface *surface = waylandSurface(window);
    if (!surface || !isActive()) {
        return nullptr;
    }

    auto *effect = get_background_effect(surface);
    m_effects.insert(window, effect);

    // Drop our handle if the window goes away before we do.
    connect(window, &QObject::destroyed, this, [this, window]() {
        if (auto *stale = m_effects.take(window)) {
            ext_background_effect_surface_v1_destroy(stale);
        }
    });

    return effect;
}

void BlurEffect::setCapsuleBlurRegion(QQuickWindow *window, const QRectF &rect, qreal radius)
{
    auto *effect = effectForWindow(window);
    wl_compositor *compositor = waylandCompositor();
    if (!effect || !compositor) {
        return;
    }

    wl_region *region = wl_compositor_create_region(compositor);

    // Clamp: a radius beyond half the height would make the caps overlap.
    radius = qBound(0.0, radius, qMin(rect.width(), rect.height()) / 2.0);

    if (radius <= 0.5) {
        wl_region_add(region, qRound(rect.x()), qRound(rect.y()),
                      qRound(rect.width()), qRound(rect.height()));
    } else {
        // Middle band: full width, between the two caps.
        wl_region_add(region,
                      qRound(rect.x()),
                      qRound(rect.y() + radius),
                      qRound(rect.width()),
                      qRound(rect.height() - 2 * radius));

        // Rounded caps, sliced horizontally. Each slice is inset by the
        // horizontal distance from the circle's centre at that height.
        for (int i = 0; i < s_capSlices; ++i) {
            const qreal y0 = radius * i / s_capSlices;
            const qreal y1 = radius * (i + 1) / s_capSlices;

            // Inset measured at the slice edge nearest the corner, so the
            // region stays inside the capsule rather than bulging past it.
            const qreal dy = radius - y0;
            const qreal inset = radius - std::sqrt(qMax(0.0, radius * radius - dy * dy));

            const int x = qRound(rect.x() + inset);
            const int w = qRound(rect.width() - 2 * inset);
            const int h = qMax(1, qRound(y1 - y0));

            // Top cap slice
            wl_region_add(region, x, qRound(rect.y() + y0), w, h);
            // Bottom cap slice, mirrored
            wl_region_add(region, x, qRound(rect.bottom() - y1), w, h);
        }
    }

    ext_background_effect_surface_v1_set_blur_region(effect, region);
    wl_region_destroy(region);

    // The blur region only takes effect on the next surface commit.
    window->requestUpdate();
}

void BlurEffect::clear(QQuickWindow *window)
{
    if (auto *effect = effectForWindow(window)) {
        ext_background_effect_surface_v1_set_blur_region(effect, nullptr);
    }
}
