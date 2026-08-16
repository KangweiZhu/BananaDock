#include "docksurface.h"

#include <QQuickWindow>
#include <QRegion>

#include <LayerShellQt/Window>

DockSurface::DockSurface(QObject *parent)
    : QObject(parent)
{
}

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
