# realvnc

JNI bindings for RealVNC's proprietary `libvncviewer.so`.

The RealVNC connection storage and cloud stuff are not included since I don't need them.

Note that while APIs aren't copyrightable, the library itself is proprietary, so artifacts built with this are not redistributable.

The library is extracted from a RealVNC APK fetched at build-time.

```bash
# fetch the apk (you can also download it yourself)
./gradlew :realvnc-jni:downloadVncApk

# build the aar
./gradlew :realvnc-jni:assembleRelease
```
