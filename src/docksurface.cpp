#include "docksurface.h"
#include "blureffect.h"
#include "screenedge.h"

#include <QQuickWindow>
#include <QRegion>

#include <LayerShellQt/Window>

DockSurface::DockSurface(QObject *parent)
    : QObject(parent)
    , m_blur(std::make_unique<BlurEffect>())
    , m_edge(std::make_unique<ScreenEdge>())
{
}

DockSurface::~DockSurface() = default;

void DockSurface::attach(QQuickWindow *window)
{
    m_window = window;
    if (!m_window) {
        return;
    }

    // Transparent background; the frosted glass is composited by KWin behind
    // the surface, not painted by us.
    m_window->setColor(Qt::transparent);

    auto *layerWindow = LayerShellQt::Window::get(m_window);
    if (!layerWindow) {
        return;
    }

    // LayerTop sits above normal windows but below the lock screen and OSDs,
    // which is the same level the macOS Dock occupies.
    layerWindow->setLayer(LayerShellQt::Window::LayerTop);

    // Anchoring to the bottom edge only lets the compositor centre the surface
    // horizontally for us -- exactly where the Dock belongs by default.
    layerWindow->setAnchors(LayerShellQt::Window::AnchorBottom);
    layerWindow->setExclusiveEdge(LayerShellQt::Window::AnchorBottom);

    layerWindow->setScope(QStringLiteral("dock"));

    // The dock must never take keyboard focus, or a single click would steal
    // input from the window the user is actually working in.
    layerWindow->setKeyboardInteractivity(LayerShellQt::Window::KeyboardInteractivityNone);

    layerWindow->setExclusiveZone(m_exclusiveZone);
}

void DockSurface::setHidden(bool hidden)
{
    if (!m_window || !m_edge) {
        return;
    }
    // The edge can only be created after the surface has its layer_surface role.
    m_edge->attach(m_window);
    m_edge->setHidden(hidden);
}

bool DockSurface::autoHideSupported() const
{
    return m_edge && m_edge->isAvailable();
}

void DockSurface::setExclusiveZone(int zone)
{
    m_exclusiveZone = zone;
    if (!m_window) {
        return;
    }
    if (auto *layerWindow = LayerShellQt::Window::get(m_window)) {
        layerWindow->setExclusiveZone(zone);
    }
}

void DockSurface::setInputRegion(int x, int y, int width, int height)
{
    if (!m_window) {
        return;
    }
    m_window->setMask(QRegion(x, y, width, height));
}

void DockSurface::setBlurRegion(qreal x, qreal y, qreal width, qreal height, qreal radius)
{
    if (!m_window || !m_blur) {
        return;
    }
    m_blur->setCapsuleBlurRegion(m_window, QRectF(x, y, width, height), radius);
}
