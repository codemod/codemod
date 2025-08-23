package com.example.modernization

// Example class with Java-style null checking patterns
class UserManager(private val userService: UserService) {
    
    fun getUserName(userId: String): String {
        // Java-style null checking
        val user = userService.findUserById(userId)
        val userSafe = user ?: return "Unknown User"
        return user.name
    }
    
    fun getUserEmail(userId: String): String {
        val user = userService.findUserById(userId)
        if (user != null) {
            val email = user.getEmail()
            val emailSafe = email ?: return "No email available"
return email
        }
        return "User not found"
    }
    
    fun getFullAddress(userId: String): String {
        val user = userService.findUserById(userId)
        val userSafe = user ?: return "No address found"
        
        val address = user.getAddress()
        val addressSafe = address ?: return "No address found"
        
        val street = address.getStreet()
        val city = address.getCity()
        val zip = address.getZipCode()
        
        if (street == null || city == null || zip == null) {
            return "Incomplete address"
        }
        
        return "$street, $city, $zip"
    }
    
    // Already using Kotlin idioms
    fun getUserProfile(userId: String): Profile {
        val user = userService.findUserById(userId) ?: return Profile("Unknown", "")
        val email = user.getEmail() ?: "No email"
        
        return Profile(user.name, email)
    }
}

class Profile(val name: String, val email: String)

interface UserService {
    fun findUserById(id: String): User?
}

interface User {
    val name: String
    fun getEmail(): String?
    fun getAddress(): Address?
}

interface Address {
    fun getStreet(): String?
    fun getCity(): String?
    fun getZipCode(): String?
}