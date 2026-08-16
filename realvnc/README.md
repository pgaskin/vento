# realvnc

JNI bindings for RealVNC's proprietary `libvncviewer.so`.

The RealVNC connection storage and cloud stuff are not included since I don't need them.

Note that while APIs aren't copyrightable, the library itself is proprietary.

The built AAR does not contain the library, only metadata which can be used to verify a library from elsewhere (e.g., extracted from another APK at runtime).

```bash
# build the aar
./gradlew :realvnc-jni:assembleRelease
```
