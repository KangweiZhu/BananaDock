cmake_minimum_required(VERSION 3.21)

project(macdock VERSION 0.1.0 LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

find_package(Qt6 6.6 REQUIRED COMPONENTS Core Gui Qml Quick WaylandClient GuiPrivate)
find_package(LayerShellQt REQUIRED)
find_package(PkgConfig REQUIRED)
pkg_check_modules(WaylandClient REQUIRED IMPORTED_TARGET wayland-client)

qt_standard_project_setup(REQUIRES 6.6)

qt_add_executable(macdock
    src/main.cpp
    src/docksurface.cpp
    src/docksurface.h
    src/blureffect.cpp
    src/blureffect.h
    src/screenedge.cpp
    src/screenedge.h
)

# Generates qwayland-ext-background-effect-v1.h -- the client bindings for the
# compositor-side blur behind the dock panel.
qt_generate_wayland_protocol_client_sources(macdock
    FILES
        /usr/share/wayland-protocols/staging/ext-background-effect/ext-background-effect-v1.xml
        /usr/share/plasma-wayland-protocols/kde-screen-edge-v1.xml
)

# Metrics is a singleton holding every pixel value. This must be declared
# before qt_add_qml_module.
set_source_files_properties(qml/Metrics.qml
    PROPERTIES QT_QML_SINGLETON_TYPE TRUE)

qt_add_qml_module(macdock
    URI MacDock
    VERSION 1.0
    QML_FILES
        qml/Main.qml
        qml/Metrics.qml
        qml/DockPanel.qml
        qml/DockItem.qml
)

target_link_libraries(macdock PRIVATE
    Qt6::Core
    Qt6::Gui
    Qt6::Qml
    Qt6::Quick
    Qt6::WaylandClient
    Qt6::GuiPrivate
    LayerShellQt::Interface
    PkgConfig::WaylandClient
)

install(TARGETS macdock RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
