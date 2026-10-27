// SPDX-FileCopyrightText: 2026 Patrick Gaskin
// SPDX-License-Identifier: GPL-3.0-or-later

//! The JNI surface every backend's library registers, written down once as
//! data: what `NativeBackend.Natives` declares, method for method, and what
//! `NativeBackend` is called back on.
//!
//! It is here, on every target, rather than only beside the code that
//! registers it, because two things read it and only one of them is on a
//! phone. `vento-codegen natives` writes each module's natives class from
//! [`NATIVES`] on the host, and [`backend`](crate::backend) registers the
//! functions under the same names on the phone, each through `jni`'s
//! `native_method!`, which checks a function against its descriptor when it
//! compiles. What ties the two lists together is
//! `scripts/check-jni-symbols.sh`: the table the library registers, read out
//! of the library, has to be the compiled class's natives, name and
//! descriptor, both ways; and the callbacks [`backend`](crate::backend) looks
//! up have to be the ones `NativeBackend` declares, both ways.
//!
//! A table rather than a Java file per module because the declarations are
//! the same for every backend. What differs is the class they are on, which
//! has to be the module's own: a `.so` registers its natives on a class, and
//! two libraries registering on one class would leave whichever came last
//! answering for both.

/// One `native` method of a module's natives class.
pub struct Native {
    /// The Java name, which is also the name registered.
    pub name: &'static str,
    /// The JNI descriptor, from which the Java types are written.
    pub descriptor: &'static str,
    /// The Java parameter names, in order, for a readable declaration.
    pub params: &'static [&'static str],
}

/// Every native `NativeBackend.Natives` declares. The handle is the first
/// argument of everything that has one: a pointer to the session, which Java
/// retires before `destroy` frees it.
pub const NATIVES: &[Native] = &[
    Native {
        name: "version",
        descriptor: "()Ljava/lang/String;",
        params: &[],
    },
    Native {
        name: "create",
        descriptor: "(Lnet/pgaskin/remotedesktop/backend/NativeBackend;Ljava/lang/String;\
                     Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;\
                     IIIILjava/lang/String;Ljava/lang/String;Z)J",
        params: &[
            "events",
            "tag",
            "address",
            "userName",
            "password",
            "options",
            "screenWidth",
            "screenHeight",
            "lastWidth",
            "lastHeight",
            "clientName",
            "filesDir",
            "forceSocketpair",
        ],
    },
    Native {
        name: "answerCredentials",
        descriptor: "(JLjava/lang/String;Ljava/lang/String;)V",
        params: &["handle", "userName", "password"],
    },
    Native {
        name: "answerTrust",
        descriptor: "(JZ)V",
        params: &["handle", "accept"],
    },
    Native {
        name: "answerPin",
        descriptor: "(JLjava/lang/String;ZZ)V",
        params: &["handle", "fingerprint", "accept", "remember"],
    },
    Native {
        name: "disconnect",
        descriptor: "(J)V",
        params: &["handle"],
    },
    Native {
        name: "destroy",
        descriptor: "(J)V",
        params: &["handle"],
    },
    Native {
        name: "pointer",
        descriptor: "(JIII)V",
        params: &["handle", "x", "y", "buttonMask"],
    },
    Native {
        name: "pointerRelative",
        descriptor: "(JIII)V",
        params: &["handle", "dx", "dy", "buttonMask"],
    },
    Native {
        name: "keyDown",
        descriptor: "(JIJ)V",
        params: &["handle", "keysym", "keyId"],
    },
    Native {
        name: "keyUp",
        descriptor: "(JJ)V",
        params: &["handle", "keyId"],
    },
    Native {
        name: "releaseAllKeys",
        descriptor: "(J)V",
        params: &["handle"],
    },
    Native {
        name: "focus",
        descriptor: "(JZ)V",
        params: &["handle", "focused"],
    },
    Native {
        name: "clipboard",
        descriptor: "(JLjava/lang/String;)V",
        params: &["handle", "text"],
    },
    Native {
        name: "setOption",
        descriptor: "(JLjava/lang/String;Ljava/lang/String;)V",
        params: &["handle", "key", "value"],
    },
    Native {
        name: "requestDesktopSize",
        descriptor: "(JII)V",
        params: &["handle", "width", "height"],
    },
    Native {
        name: "requestDisplay",
        descriptor: "(JI)V",
        params: &["handle", "index"],
    },
    Native {
        name: "setMuted",
        descriptor: "(JZ)V",
        params: &["handle", "muted"],
    },
    Native {
        name: "setSoundEnabled",
        descriptor: "(JZ)V",
        params: &["handle", "enabled"],
    },
    Native {
        name: "facts",
        descriptor: "(J)[I",
        params: &["handle"],
    },
    Native {
        name: "info",
        descriptor: "(J)[Ljava/lang/String;",
        params: &["handle"],
    },
    Native {
        name: "traffic",
        descriptor: "(J)[J",
        params: &["handle"],
    },
    Native {
        name: "soundLine",
        descriptor: "(J)Ljava/lang/String;",
        params: &["handle"],
    },
    Native {
        name: "readRegion",
        descriptor: "(JIIIILandroid/graphics/Bitmap;II)Z",
        params: &["handle", "x", "y", "width", "height", "dst", "dstX", "dstY"],
    },
    Native {
        name: "readThumbnail",
        descriptor: "(JILandroid/graphics/Bitmap;)Z",
        params: &["handle", "step", "dst"],
    },
];

/// The Java types a descriptor names, parameters and return, as a
/// declaration writes them; `None` for a descriptor this table would never
/// hold.
pub fn java_types(descriptor: &str) -> Option<(Vec<String>, String)> {
    let inner = descriptor.strip_prefix('(')?;
    let (args, ret) = inner.split_once(')')?;
    let mut params = Vec::new();
    let mut rest = args;
    while !rest.is_empty() {
        let (ty, tail) = one_type(rest)?;
        params.push(ty);
        rest = tail;
    }
    let (ret, tail) = one_type(ret)?;
    tail.is_empty().then_some((params, ret))
}

fn one_type(s: &str) -> Option<(String, &str)> {
    let (first, tail) = (s.chars().next()?, &s[1..]);
    let primitive = match first {
        'Z' => "boolean",
        'B' => "byte",
        'C' => "char",
        'S' => "short",
        'I' => "int",
        'J' => "long",
        'F' => "float",
        'D' => "double",
        'V' => "void",
        '[' => {
            let (element, tail) = one_type(tail)?;
            return Some((element + "[]", tail));
        }
        'L' => {
            let (class, tail) = tail.split_once(';')?;
            return Some((class.replace(['/', '$'], "."), tail));
        }
        _ => return None,
    };
    Some((primitive.to_string(), tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_native_has_a_name_for_each_parameter() {
        for native in NATIVES {
            let (params, _) = java_types(native.descriptor)
                .unwrap_or_else(|| panic!("{}: bad descriptor", native.name));
            assert_eq!(params.len(), native.params.len(), "{}", native.name);
        }
    }

    #[test]
    fn descriptors_read_as_java_types() {
        let (params, ret) = java_types("(JILandroid/graphics/Bitmap;[I)[Ljava/lang/String;").unwrap();
        assert_eq!(params, ["long", "int", "android.graphics.Bitmap", "int[]"]);
        assert_eq!(ret, "java.lang.String[]");
        assert!(java_types("(Q)V").is_none());
    }
}
